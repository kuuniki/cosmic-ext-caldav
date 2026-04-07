use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::{header, Client};
use std::sync::LazyLock;
use uuid::Uuid;
use zeroize::Zeroize;

/// Hard cap on CalDAV response bodies — prevents a malicious or misconfigured
/// server from exhausting process memory with an unbounded stream.
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024; // 10 MiB

/// Reads a response body up to `MAX_BODY_BYTES`, returning an error if the
/// stream exceeds the limit before EOF.  Uses chunked streaming so memory is
/// bounded even when no `Content-Length` header is present.
async fn read_bounded_body(resp: reqwest::Response) -> Result<String, String> {
    // Fast-reject when Content-Length is already over the cap.
    if resp
        .content_length()
        .map_or(false, |n| n > MAX_BODY_BYTES as u64)
    {
        return Err("Server response too large (> 10 MiB)".to_string());
    }
    let mut buf = Vec::with_capacity(65_536);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            eprintln!("CalDAV read body error: {}", e);
            "Failed to read server response.".to_string()
        })?;
        buf.extend_from_slice(&chunk);
        if buf.len() > MAX_BODY_BYTES {
            return Err("Server response too large (> 10 MiB)".to_string());
        }
    }
    String::from_utf8(buf).map_err(|_| "Server response is not valid UTF-8.".to_string())
}

// Static HTTP method constants — defined once, cloned at each call site.
// `from_bytes` is infallible for these well-known ASCII strings; the
// `expect` message is intentionally descriptive for any future mis-edit.
static PROPFIND: LazyLock<reqwest::Method> = LazyLock::new(|| {
    reqwest::Method::from_bytes(b"PROPFIND").expect("PROPFIND is a valid HTTP method")
});
static REPORT: LazyLock<reqwest::Method> = LazyLock::new(|| {
    reqwest::Method::from_bytes(b"REPORT").expect("REPORT is a valid HTTP method")
});

#[derive(Debug, Clone)]
pub struct CalendarEvent {
    /// Stable event UID from the VCALENDAR payload.
    pub uid: String,
    /// Resource href returned by the CalDAV server for this event.
    pub href: String,
    /// Optional entity tag used for safe update/delete requests.
    pub etag: Option<String>,
    pub summary: String,
    pub start: Option<DateTime<Utc>>,
    #[allow(dead_code)]
    pub end: Option<DateTime<Utc>>,
    /// Displayed in the settings app event card; not shown in the compact applet.
    #[allow(dead_code)]
    pub description: Option<String>,
    #[allow(dead_code)]
    pub location: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Calendar {
    pub href: String,
    /// Displayed in the settings app calendar list; not used in the applet.
    #[allow(dead_code)]
    pub display_name: String,
    /// Parsed for future use; not yet applied to the UI.
    #[allow(dead_code)]
    pub color: Option<String>,
}

#[derive(Clone)]
pub struct CalDavClient {
    base_url: String,
    username: String,
    password: String,
    client: Client,
}

impl Drop for CalDavClient {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

impl CalDavClient {
    pub fn new(base_url: String, username: String, password: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        let base_url = base_url.trim_end_matches('/').to_string();
        Self {
            base_url,
            username,
            password,
            client,
        }
    }

    fn caldav_url(&self) -> String {
        // Google and Outlook URLs are already full CalDAV paths (auto-filled in UI)
        // Only append Nextcloud's path if it's not already a full CalDAV URL
        if self.base_url.contains("google.com/calendar")
            || self.base_url.contains("outlook.office365.com")
            || self.base_url.contains("/remote.php/dav/")
            || self.base_url.contains("/dav/")
        {
            format!("{}/", self.base_url.trim_end_matches('/'))
        } else {
            // Encode '/' in the username so a username like "a/b" cannot traverse
            // outside the calendars directory.  Other characters (e.g. '@') are
            // legal in URL path segments and do not need encoding here.
            let encoded_username = self.username.replace('/', "%2F");
            format!(
                "{}/remote.php/dav/calendars/{}/",
                self.base_url, encoded_username
            )
        }
    }

    #[allow(dead_code)]
    pub async fn test_connection(&self) -> Result<(), String> {
        enforce_https(&self.base_url)?;
        let url = self.caldav_url();
        let resp = self.client
            .request(PROPFIND.clone(), &url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "0")
            .header(header::CONTENT_TYPE, "application/xml")
            .body(r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:displayname/></d:prop></d:propfind>"#)
            .send()
            .await
            .map_err(|e| {
                eprintln!("CalDAV test_connection error: {}", e);
                "Could not reach the server. Check the URL and your network connection.".to_string()
            })?;

        if resp.status().is_success() || resp.status().as_u16() == 207 {
            Ok(())
        } else if resp.status().as_u16() == 401 {
            Err("Authentication failed. Check your username and password.".to_string())
        } else {
            Err(format!("Server returned HTTP {}", resp.status().as_u16()))
        }
    }

    pub async fn get_calendars(&self) -> Result<Vec<Calendar>, String> {
        enforce_https(&self.base_url)?;
        let url = self.caldav_url();
        let body = r#"<?xml version="1.0"?>
<d:propfind xmlns:d="DAV:" xmlns:cs="http://calendarserver.org/ns/" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="http://apple.com/ns/ical/">
  <d:prop>
    <d:displayname/>
    <a:calendar-color/>
    <c:supported-calendar-component-set/>
  </d:prop>
</d:propfind>"#;

        let resp = self
            .client
            .request(PROPFIND.clone(), &url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "1")
            .header(header::CONTENT_TYPE, "application/xml")
            .body(body)
            .send()
            .await
            .map_err(|e| {
                eprintln!("CalDAV get_calendars error: {}", e);
                "Could not fetch calendars. Check your network connection.".to_string()
            })?;

        let text = read_bounded_body(resp).await.map_err(|e| {
            eprintln!("CalDAV get_calendars: {}", e);
            e
        })?;
        parse_calendars(&text)
    }

    pub async fn get_events(&self, calendar_href: &str) -> Result<Vec<CalendarEvent>, String> {
        enforce_https(&self.base_url)?;
        let url = validate_server_href(calendar_href, &self.base_url)?;

        let now = chrono::Utc::now();
        // Fetch 30 days of past events and 365 days ahead so future months are visible
        let start = (now - chrono::Duration::days(30)).format("%Y%m%dT000000Z");
        let end = (now + chrono::Duration::days(365)).format("%Y%m%dT235959Z");
        let body = format!(
            r#"<?xml version="1.0"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:time-range start="{start}" end="{end}"/>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#
        );

        let resp = self
            .client
            .request(REPORT.clone(), &url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "1")
            .header(header::CONTENT_TYPE, "application/xml")
            .body(body)
            .send()
            .await
            .map_err(|e| {
                eprintln!("CalDAV get_events error: {}", e);
                "Could not fetch events. Check your network connection.".to_string()
            })?;

        let text = read_bounded_body(resp).await.map_err(|e| {
            eprintln!("CalDAV get_events: {}", e);
            e
        })?;
        parse_events(&text)
    }

    /// Called from the applet binary (SubmitEvent).  The settings binary has no
    /// event-creation flow so the compiler flags this as dead code for that target.
    #[allow(dead_code)]
    pub async fn create_event(
        &self,
        calendar_href: &str,
        summary: &str,
        start: chrono::DateTime<chrono::Local>,
        end: chrono::DateTime<chrono::Local>,
        location: &str,
        description: &str,
        reminder_mins: i32,
    ) -> Result<(), String> {
        enforce_https(&self.base_url)?;
        if end <= start {
            return Err("End date/time must be after start date/time".to_string());
        }
        let uid = generate_uid();
        let ical = build_ical_event(&uid, summary, start, end, location, description, reminder_mins);
        let base_href = validate_server_href(calendar_href, &self.base_url)?;
        let url = format!("{}/{}.ics", base_href.trim_end_matches('/'), uid);
        let resp = self
            .client
            .put(&url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Content-Type", "text/calendar; charset=utf-8")
            .body(ical)
            .send()
            .await
            .map_err(|e| {
                eprintln!("CalDAV create_event error: {}", e);
                "Could not create event. Check your network connection.".to_string()
            })?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("Server error: HTTP {}", resp.status().as_u16()))
        }
    }

    pub async fn update_event(
        &self,
        event_href: &str,
        etag: Option<&str>,
        uid: &str,
        summary: &str,
        start: chrono::DateTime<chrono::Local>,
        end: chrono::DateTime<chrono::Local>,
        location: &str,
        description: &str,
        reminder_mins: i32,
    ) -> Result<(), String> {
        enforce_https(&self.base_url)?;
        if end <= start {
            return Err("End date/time must be after start date/time".to_string());
        }
        let url = validate_server_href(event_href, &self.base_url)?;
        let ical = build_ical_event(uid, summary, start, end, location, description, reminder_mins);
        let mut req = self
            .client
            .put(&url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Content-Type", "text/calendar; charset=utf-8");
        if let Some(etag) = etag.filter(|s| !s.is_empty()) {
            req = req.header(header::IF_MATCH, etag);
        }
        let resp = req.body(ical).send().await.map_err(|e| {
            eprintln!("CalDAV update_event error: {}", e);
            "Could not update event. Check your network connection.".to_string()
        })?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("Server error: HTTP {}", resp.status().as_u16()))
        }
    }

    pub async fn delete_event(&self, event_href: &str, etag: Option<&str>) -> Result<(), String> {
        enforce_https(&self.base_url)?;
        let url = validate_server_href(event_href, &self.base_url)?;
        let mut req = self
            .client
            .delete(&url)
            .basic_auth(&self.username, Some(&self.password));
        if let Some(etag) = etag.filter(|s| !s.is_empty()) {
            req = req.header(header::IF_MATCH, etag);
        }
        let resp = req.send().await.map_err(|e| {
            eprintln!("CalDAV delete_event error: {}", e);
            "Could not delete event. Check your network connection.".to_string()
        })?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("Server error: HTTP {}", resp.status().as_u16()))
        }
    }
}

fn parse_calendars(xml: &str) -> Result<Vec<Calendar>, String> {
    let mut calendars = Vec::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut current_href = String::new();
    let mut current_name = String::new();
    let mut current_color = None;
    let mut in_href = false;
    let mut in_displayname = false;
    let mut in_color = false;
    let mut in_response = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                match e.local_name().as_ref() {
                    b"response" => {
                        in_response = true;
                        current_href.clear();
                        current_name.clear();
                        current_color = None;
                    }
                    b"href" if in_response => in_href = true,
                    b"displayname" => in_displayname = true,
                    b"calendar-color" => in_color = true,
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if in_href {
                    current_href = text;
                    in_href = false;
                } else if in_displayname {
                    current_name = text;
                    in_displayname = false;
                } else if in_color {
                    current_color = Some(text);
                    in_color = false;
                }
            }
            Ok(Event::End(e)) => {
                match e.local_name().as_ref() {
                    b"response" if in_response => {
                        if current_href.ends_with('/') && !current_name.is_empty() {
                            calendars.push(Calendar {
                                href: current_href.clone(),
                                display_name: current_name.clone(),
                                color: current_color.clone(),
                            });
                        }
                        in_response = false;
                    }
                    // Reset flags on End so an empty element (e.g. <displayname/>)
                    // does not bleed its flag into the next sibling's text content.
                    b"href" => in_href = false,
                    b"displayname" => in_displayname = false,
                    b"calendar-color" => in_color = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error: {}", e)),
            _ => {}
        }
    }
    Ok(calendars)
}

fn parse_events(xml: &str) -> Result<Vec<CalendarEvent>, String> {
    let mut events = Vec::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut current_href = String::new();
    let mut current_etag: Option<String> = None;
    let mut calendar_data = String::new();

    let mut in_response = false;
    let mut in_href = false;
    let mut in_getetag = false;
    let mut in_calendar_data = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"response" => {
                    in_response = true;
                    current_href.clear();
                    current_etag = None;
                    calendar_data.clear();
                }
                b"href" if in_response => in_href = true,
                b"getetag" if in_response => in_getetag = true,
                b"calendar-data" if in_response => {
                    in_calendar_data = true;
                    calendar_data.clear();
                }
                _ => {}
            },
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if in_href {
                    current_href = text;
                    in_href = false;
                } else if in_getetag {
                    current_etag = Some(text);
                    in_getetag = false;
                } else if in_calendar_data {
                    calendar_data.push_str(&text);
                }
            }
            Ok(Event::CData(e)) => {
                if in_calendar_data {
                    calendar_data.push_str(&String::from_utf8_lossy(&e));
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"response" if in_response => {
                    if !calendar_data.is_empty() {
                        events.extend(parse_ical_events(
                            &calendar_data,
                            &current_href,
                            current_etag.as_deref(),
                        ));
                    }
                    in_response = false;
                    in_href = false;
                    in_getetag = false;
                    in_calendar_data = false;
                }
                b"href" => in_href = false,
                b"getetag" => in_getetag = false,
                b"calendar-data" => in_calendar_data = false,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error: {}", e)),
            _ => {}
        }
    }
    Ok(events)
}

fn parse_ical_events(ical: &str, href: &str, etag: Option<&str>) -> Vec<CalendarEvent> {
    #[derive(Default)]
    struct EventBuilder {
        uid: String,
        summary: String,
        dtstart: Option<DateTime<Utc>>,
        dtend: Option<DateTime<Utc>>,
        description: Option<String>,
        location: Option<String>,
    }

    fn finalize(builder: EventBuilder, href: &str, etag: Option<&str>) -> Option<CalendarEvent> {
        if builder.uid.is_empty() && builder.summary.is_empty() {
            return None;
        }

        Some(CalendarEvent {
            uid: builder.uid,
            href: href.to_string(),
            etag: etag.map(|s| s.to_string()),
            summary: builder.summary,
            start: builder.dtstart,
            end: builder.dtend,
            description: builder.description,
            location: builder.location,
        })
    }

    let mut events = Vec::new();
    let mut current: Option<EventBuilder> = None;

    for line in unfold_ical_lines(ical) {
        if line == "BEGIN:VEVENT" {
            current = Some(EventBuilder::default());
            continue;
        }

        if line == "END:VEVENT" {
            if let Some(builder) = current.take().and_then(|b| finalize(b, href, etag)) {
                events.push(builder);
            }
            continue;
        }

        let Some(event) = current.as_mut() else {
            continue;
        };

        if let Some(val) = line.strip_prefix("UID:") {
            event.uid = val.to_string();
        } else if let Some(val) = line.strip_prefix("SUMMARY:") {
            event.summary = unescape_ical_text(val);
        } else if line.starts_with("DTSTART") {
            event.dtstart = parse_ical_date(&line);
        } else if line.starts_with("DTEND") {
            event.dtend = parse_ical_date(&line);
        } else if let Some(val) = line.strip_prefix("DESCRIPTION:") {
            event.description = Some(unescape_ical_text(val));
        } else if let Some(val) = line.strip_prefix("LOCATION:") {
            event.location = Some(unescape_ical_text(val));
        }
    }

    events
}

fn unfold_ical_lines(ical: &str) -> Vec<String> {
    let mut unfolded: Vec<String> = Vec::new();

    for line in ical.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = unfolded.last_mut() {
                last.push_str(line.trim_start_matches([' ', '\t']));
            }
            continue;
        }

        unfolded.push(line.trim_end_matches('\r').to_string());
    }

    unfolded
}

fn build_ical_event(
    uid: &str,
    summary: &str,
    start: chrono::DateTime<chrono::Local>,
    end: chrono::DateTime<chrono::Local>,
    location: &str,
    description: &str,
    reminder_mins: i32,
) -> String {
    let fmt = "%Y%m%dT%H%M%S";
    let alarm = if reminder_mins > 0 {
        format!(
            "BEGIN:VALARM\r\nTRIGGER:-PT{}M\r\nACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\n",
            reminder_mins
        )
    } else {
        String::new()
    };
    let loc_line = if !location.is_empty() {
        format!("LOCATION:{}\r\n", escape_ical_text(location))
    } else {
        String::new()
    };
    let desc_line = if !description.is_empty() {
        format!("DESCRIPTION:{}\r\n", escape_ical_text(description))
    } else {
        String::new()
    };

    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//cosmic-caldav//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{summary}\r\n{loc}{desc}DTSTART:{start}\r\nDTEND:{end}\r\n{alarm}END:VEVENT\r\nEND:VCALENDAR\r\n",
        uid = uid,
        summary = escape_ical_text(summary),
        loc = loc_line,
        desc = desc_line,
        start = start.format(fmt),
        end = end.format(fmt),
        alarm = alarm,
    )
}

/// Only called from `create_event` which is compiled into the applet binary;
/// the settings binary does not create events so both are dead from its perspective.
#[allow(dead_code)]
fn escape_ical_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace("\n", "\\n")
        .replace("\r", "")
}

fn unescape_ical_text(text: &str) -> String {
    // Single-pass parser so that `\\n` (literal backslash + n) is not
    // incorrectly converted to a newline.  The previous chained-replace
    // approach processed `\\` last, which meant `\\n` was first turned
    // into `\` + newline instead of the correct `\n`.
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(',') => out.push(','),
                Some(';') => out.push(';'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn parse_ical_date(line: &str) -> Option<DateTime<Utc>> {
    // Extract TZID if present e.g. DTSTART;TZID=America/New_York:20240315T090000
    let tzid = if let Some(params) = line.split(':').next() {
        params
            .split(';')
            .find_map(|p| p.strip_prefix("TZID=").map(|tz| tz.to_string()))
    } else {
        None
    };

    let val = line.split(':').last()?.trim();

    // UTC timestamp ending in Z
    if val.ends_with('Z') && val.len() >= 15 {
        return chrono::NaiveDateTime::parse_from_str(&val[..15], "%Y%m%dT%H%M%S")
            .ok()
            .map(|ndt| ndt.and_utc());
    }

    // Date-only value
    if val.len() == 8 {
        return chrono::NaiveDate::parse_from_str(val, "%Y%m%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|ndt| ndt.and_utc());
    }

    // Naive datetime - try to apply TZID if present
    if val.len() >= 15 {
        let ndt = chrono::NaiveDateTime::parse_from_str(&val[..15], "%Y%m%dT%H%M%S").ok()?;
        if let Some(tz_name) = tzid {
            if let Ok(tz) = tz_name.parse::<chrono_tz::Tz>() {
                use chrono::TimeZone;
                return tz
                    .from_local_datetime(&ndt)
                    .earliest()
                    .map(|dt| dt.to_utc());
            }
        }
        // Fallback: treat as UTC
        return Some(ndt.and_utc());
    }

    None
}

/// Generates a cryptographically random UUID v4 for use as a CalDAV event UID.
#[allow(dead_code)]
fn generate_uid() -> String {
    Uuid::new_v4().simple().to_string()
}

/// Enforces that the configured CalDAV server URL uses HTTPS.
/// `http://localhost` and loopback addresses are permitted for local development.
fn enforce_https(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        return Ok(());
    }
    // Allow plain HTTP only for genuine loopback addresses.  The previous
    // `starts_with("http://localhost")` check also matched hostnames like
    // `http://localhost.evil.com`, allowing an attacker-controlled URL to
    // bypass HTTPS enforcement and receive credentials in plaintext.
    if url.starts_with("http://") {
        let after_scheme = &url["http://".len()..];
        let host_port = after_scheme.split('/').next().unwrap_or("");
        let host = host_port.split(':').next().unwrap_or("");
        if host == "localhost" || host == "127.0.0.1" || host == "[::1]" {
            return Ok(());
        }
    }
    Err("CalDAV URL must use HTTPS to protect your credentials. \
         Please change http:// to https://"
        .to_string())
}

/// Validates a href returned by the CalDAV server before using it in a request.
///
/// Absolute hrefs must use http/https and must point to the same host as the
/// configured base URL — this prevents a malicious server from redirecting
/// requests to an attacker-controlled host.  Relative hrefs are combined with
/// `base_url` without further restriction (the server already controls the path).
fn validate_server_href(href: &str, base_url: &str) -> Result<String, String> {
    let base = reqwest::Url::parse(base_url)
        .map_err(|_| "The configured server URL is invalid".to_string())?;

    let resolved = if href.starts_with("http://") || href.starts_with("https://") {
        reqwest::Url::parse(href)
            .map_err(|_| "Server returned an invalid URL".to_string())?
    } else {
        base.join(href)
            .map_err(|_| "Server returned an invalid URL".to_string())?
    };

    if resolved.scheme() != "http" && resolved.scheme() != "https" {
        return Err("Server href uses an unsupported URL scheme".to_string());
    }

    if resolved.scheme() != base.scheme() {
        return Err(
            "Server href uses a different scheme than configured — request blocked".to_string(),
        );
    }

    if resolved.host() != base.host()
        || resolved.port_or_known_default() != base.port_or_known_default()
    {
        return Err(
            "Server href points to a different host/port than configured — request blocked"
                .to_string(),
        );
    }

    Ok(resolved.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_ical_events;

    #[test]
    fn parse_ical_events_parses_all_vevents() {
        let ical = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:first\r\nSUMMARY:First\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:second\r\nSUMMARY:Second\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        let events = parse_ical_events(ical, "/calendar/test.ics", Some("\"etag\""));

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].uid, "first");
        assert_eq!(events[1].uid, "second");
    }

    #[test]
    fn parse_ical_events_unfolds_folded_lines() {
        let ical = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:u\r\nSUMMARY:Long line\r\nDESCRIPTION:First part\r\n second part\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        let events = parse_ical_events(ical, "/calendar/test.ics", Some("\"etag\""));

        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].description.as_deref(),
            Some("First partsecond part")
        );
    }

    #[test]
    fn enforce_https_accepts_https() {
        use super::enforce_https;
        assert!(enforce_https("https://nextcloud.example.com").is_ok());
        assert!(enforce_https("https://www.google.com/calendar/dav/user/events/").is_ok());
    }

    #[test]
    fn enforce_https_allows_localhost_http() {
        use super::enforce_https;
        assert!(enforce_https("http://localhost:8080").is_ok());
        assert!(enforce_https("http://127.0.0.1:5232").is_ok());
    }

    #[test]
    fn enforce_https_rejects_plain_http() {
        use super::enforce_https;
        assert!(enforce_https("http://nextcloud.example.com").is_err());
    }

    #[test]
    fn validate_server_href_rejects_different_host() {
        use super::validate_server_href;
        let result = validate_server_href(
            "https://evil.example.com/calendars/user/",
            "https://safe.example.com",
        );
        assert!(result.is_err());
    }

    #[test]
    fn validate_server_href_accepts_same_host() {
        use super::validate_server_href;
        let result = validate_server_href(
            "https://safe.example.com/remote.php/dav/calendars/user/cal/",
            "https://safe.example.com",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn validate_server_href_rejects_different_port() {
        use super::validate_server_href;
        let result = validate_server_href(
            "https://safe.example.com:8080/calendars/user/",
            "https://safe.example.com",
        );
        assert!(result.is_err(), "different port should be rejected");
    }

    #[test]
    fn validate_server_href_accepts_explicit_default_port() {
        use super::validate_server_href;
        // https://host:443 and https://host are the same server
        let result = validate_server_href(
            "https://safe.example.com:443/calendars/user/",
            "https://safe.example.com",
        );
        assert!(
            result.is_ok(),
            "explicit default port should match implicit default"
        );
    }

    #[test]
    fn validate_server_href_combines_relative() {
        use super::validate_server_href;
        let result = validate_server_href(
            "/remote.php/dav/calendars/user/personal/",
            "https://safe.example.com",
        );
        assert_eq!(
            result.unwrap(),
            "https://safe.example.com/remote.php/dav/calendars/user/personal/"
        );
    }

    #[test]
    fn validate_server_href_combines_google_relative_with_origin_only() {
        use super::validate_server_href;
        let result = validate_server_href(
            "/calendar/dav/user@gmail.com/events/",
            "https://www.google.com/calendar/dav/user@gmail.com/events/",
        );
        assert_eq!(result.unwrap(), "https://www.google.com/calendar/dav/user@gmail.com/events/");
    }

    #[test]
    fn validate_server_href_combines_relative_preserving_non_default_port() {
        use super::validate_server_href;
        let result = validate_server_href(
            "/dav/calendars/user/personal/",
            "https://safe.example.com:8443/base/path/",
        );
        assert_eq!(
            result.unwrap(),
            "https://safe.example.com:8443/dav/calendars/user/personal/"
        );
    }

    #[test]
    fn validate_server_href_resolves_path_relative_href() {
        use super::validate_server_href;
        let result = validate_server_href(
            "events/test.ics",
            "https://safe.example.com/calendars/user/",
        );
        assert_eq!(result.unwrap(), "https://safe.example.com/calendars/user/events/test.ics");
    }

    #[test]
    fn validate_server_href_rejects_network_path_reference_to_different_host() {
        use super::validate_server_href;
        let result = validate_server_href(
            "//evil.example/steal",
            "https://safe.example.com/calendar/dav/user@gmail.com/events/",
        );
        assert!(result.is_err(), "network-path reference should be rejected");
    }

    #[test]
    fn enforce_https_rejects_localhost_subdomain() {
        use super::enforce_https;
        // "http://localhost.evil.com" must NOT be treated as localhost
        assert!(
            enforce_https("http://localhost.evil.com").is_err(),
            "localhost.evil.com should be rejected"
        );
        assert!(
            enforce_https("http://localhost.evil.com:8080/path").is_err(),
            "localhost.evil.com with port should be rejected"
        );
    }

    #[test]
    fn enforce_https_allows_localhost_with_path() {
        use super::enforce_https;
        assert!(enforce_https("http://localhost/dav").is_ok());
        assert!(enforce_https("http://localhost:5232/dav").is_ok());
        assert!(enforce_https("http://[::1]:8080/calendars").is_ok());
    }

    #[test]
    fn validate_server_href_rejects_scheme_downgrade() {
        use super::validate_server_href;
        // A malicious server returning http:// href when base is https://
        // would cause credentials to be sent in plaintext.
        let result = validate_server_href(
            "http://safe.example.com/calendars/user/",
            "https://safe.example.com",
        );
        assert!(
            result.is_err(),
            "http href with https base should be rejected"
        );
    }

    #[test]
    fn unescape_ical_text_handles_escaped_backslash_before_n() {
        use super::unescape_ical_text;
        // \\n in iCal means literal backslash followed by literal n
        assert_eq!(unescape_ical_text("\\\\n"), "\\n");
        // \n in iCal means a newline
        assert_eq!(unescape_ical_text("\\n"), "\n");
        // Mixed: literal backslash, then a real newline escape
        assert_eq!(unescape_ical_text("\\\\\\n"), "\\\n");
        // Basic unescaping still works
        assert_eq!(unescape_ical_text("hello\\, world"), "hello, world");
        assert_eq!(unescape_ical_text("a\\;b"), "a;b");
    }
}
