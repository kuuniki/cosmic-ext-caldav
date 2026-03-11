use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::{header, Client};
use std::sync::LazyLock;
use uuid::Uuid;

/// Hard cap on CalDAV response bodies — prevents a malicious or misconfigured
/// server from exhausting process memory with an unbounded stream.
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024; // 10 MiB

/// Reads a response body up to `MAX_BODY_BYTES`, returning an error if the
/// stream exceeds the limit before EOF.  Uses chunked streaming so memory is
/// bounded even when no `Content-Length` header is present.
async fn read_bounded_body(resp: reqwest::Response) -> Result<String, String> {
    // Fast-reject when Content-Length is already over the cap.
    if resp.content_length().map_or(false, |n| n > MAX_BODY_BYTES as u64) {
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
    pub uid: String,
    pub summary: String,
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    pub description: Option<String>,
    pub location: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Calendar {
    pub href: String,
    pub display_name: String,
    pub color: Option<String>,
}

#[derive(Clone)]
pub struct CalDavClient {
    base_url: String,
    username: String,
    password: String,
    client: Client,
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

        let text = read_bounded_body(resp).await
            .map_err(|e| { eprintln!("CalDAV get_calendars: {}", e); e })?;
        parse_calendars(&text)
    }

    pub async fn get_events(&self, calendar_href: &str) -> Result<Vec<CalendarEvent>, String> {
        enforce_https(&self.base_url)?;
        let url = validate_server_href(calendar_href, &self.base_url)?;

        let now = chrono::Utc::now();
        // Fetch 7 days of past events and 60 days ahead — limits bandwidth for history
        let start = (now - chrono::Duration::days(7)).format("%Y%m%dT000000Z");
        let end = (now + chrono::Duration::days(60)).format("%Y%m%dT235959Z");
        let body = format!(r#"<?xml version="1.0"?>
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
</c:calendar-query>"#);

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

        let text = read_bounded_body(resp).await
            .map_err(|e| { eprintln!("CalDAV get_events: {}", e); e })?;
        parse_events(&text)
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
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                match name.as_str() {
                    "response" => {
                        in_response = true;
                        current_href.clear();
                        current_name.clear();
                        current_color = None;
                    }
                    "href" if in_response => in_href = true,
                    "displayname" => in_displayname = true,
                    "calendar-color" => in_color = true,
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if in_href {
                    current_href = text.clone();
                    in_href = false;
                }
                if in_displayname {
                    current_name = text.clone();
                    in_displayname = false;
                }
                if in_color {
                    current_color = Some(text);
                    in_color = false;
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                match name.as_str() {
                    "response" if in_response => {
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
                    "href" => in_href = false,
                    "displayname" => in_displayname = false,
                    "calendar-color" => in_color = false,
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
    let mut in_calendar_data = false;
    let mut calendar_data = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if name == "calendar-data" {
                    in_calendar_data = true;
                    calendar_data.clear();
                }
            }
            Ok(Event::Text(e)) => {
                if in_calendar_data {
                    calendar_data.push_str(&e.unescape().unwrap_or_default());
                }
            }
            Ok(Event::CData(e)) => {
                if in_calendar_data {
                    calendar_data.push_str(&String::from_utf8_lossy(&e));
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if name == "calendar-data" && in_calendar_data {
                    in_calendar_data = false;
                    events.extend(parse_ical_events(&calendar_data));
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error: {}", e)),
            _ => {}
        }
    }
    Ok(events)
}

fn parse_ical_events(ical: &str) -> Vec<CalendarEvent> {
    #[derive(Default)]
    struct EventBuilder {
        uid: String,
        summary: String,
        dtstart: Option<DateTime<Utc>>,
        dtend: Option<DateTime<Utc>>,
        description: Option<String>,
        location: Option<String>,
    }

    fn finalize(builder: EventBuilder) -> Option<CalendarEvent> {
        if builder.uid.is_empty() && builder.summary.is_empty() {
            return None;
        }

        Some(CalendarEvent {
            uid: builder.uid,
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
            if let Some(builder) = current.take().and_then(finalize) {
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

fn escape_ical_text(s: &str) -> String {
    s.replace('\\', "\\\\")
     .replace(';', "\\;")
     .replace(',', "\\,")
     .replace("\n", "\\n")
     .replace("\r", "")
}

fn unescape_ical_text(text: &str) -> String {
    text.replace("\\n", "\n")
        .replace("\\N", "\n")
        .replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
}

fn parse_ical_date(line: &str) -> Option<DateTime<Utc>> {
    // Extract TZID if present e.g. DTSTART;TZID=America/New_York:20240315T090000
    let tzid = if let Some(params) = line.split(':').next() {
        params.split(';').find_map(|p| {
            p.strip_prefix("TZID=").map(|tz| tz.to_string())
        })
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
                return tz.from_local_datetime(&ndt).earliest().map(|dt| dt.to_utc());
            }
        }
        // Fallback: treat as UTC
        return Some(ndt.and_utc());
    }

    None
}

impl CalDavClient {
    pub async fn create_event(
        &self,
        calendar_href: &str,
        summary: &str,
        date: chrono::NaiveDate,
        hour: u32,
        minute: u32,
        duration_mins: u32,
        location: &str,
        description: &str,
        reminder_mins: i32,
    ) -> Result<(), String> {
        enforce_https(&self.base_url)?;
        use chrono::TimeZone;
        let uid = generate_uid();
        let naive = date.and_hms_opt(hour, minute, 0)
            .ok_or_else(|| "Invalid time values".to_string())?;
        let start = chrono::Local
            .from_local_datetime(&naive)
            .earliest()
            .ok_or_else(|| "Invalid or ambiguous local time (DST gap)".to_string())?;
        let end = start + chrono::Duration::minutes(duration_mins as i64);
        let fmt = "%Y%m%dT%H%M%S";
        let alarm = if reminder_mins > 0 {
            format!("BEGIN:VALARM\r\nTRIGGER:-PT{}M\r\nACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\n", reminder_mins)
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
        let ical = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//cosmic-caldav//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{summary}\r\n{loc}{desc}DTSTART:{start}\r\nDTEND:{end}\r\n{alarm}END:VEVENT\r\nEND:VCALENDAR\r\n",
            uid = uid,
            summary = escape_ical_text(summary),
            loc = loc_line,
            desc = desc_line,
            start = start.format(fmt),
            end = end.format(fmt),
            alarm = alarm,
        );
        // Validate the server-provided href before constructing the PUT URL
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
        if resp.status().is_success() || resp.status().as_u16() == 201 {
            Ok(())
        } else {
            Err(format!("Server error: HTTP {}", resp.status().as_u16()))
        }
    }
}

/// Generates a cryptographically random UUID v4 for use as a CalDAV event UID.
fn generate_uid() -> String {
    Uuid::new_v4().simple().to_string()
}

/// Enforces that the configured CalDAV server URL uses HTTPS.
/// `http://localhost` and loopback addresses are permitted for local development.
fn enforce_https(url: &str) -> Result<(), String> {
    if url.starts_with("https://")
        || url.starts_with("http://localhost")
        || url.starts_with("http://127.0.0.1")
        || url.starts_with("http://[::1]")
    {
        Ok(())
    } else {
        Err(
            "CalDAV URL must use HTTPS to protect your credentials. \
             Please change http:// to https://"
                .to_string(),
        )
    }
}

/// Validates a href returned by the CalDAV server before using it in a request.
///
/// Absolute hrefs must use http/https and must point to the same host as the
/// configured base URL — this prevents a malicious server from redirecting
/// requests to an attacker-controlled host.  Relative hrefs are combined with
/// `base_url` without further restriction (the server already controls the path).
fn validate_server_href(href: &str, base_url: &str) -> Result<String, String> {
    if href.starts_with("http://") || href.starts_with("https://") {
        let parsed = reqwest::Url::parse(href)
            .map_err(|_| "Server returned an invalid URL".to_string())?;
        // The outer starts_with guard already guarantees http/https scheme;
        // no redundant inner scheme check needed.
        let base = reqwest::Url::parse(base_url)
            .map_err(|_| "The configured server URL is invalid".to_string())?;
        // Compare host AND port — a different port could be a different service
        // on the same machine.  port_or_known_default() normalises omitted ports
        // (e.g. https://host == https://host:443) so the comparison is consistent.
        if parsed.host() != base.host()
            || parsed.port_or_known_default() != base.port_or_known_default()
        {
            return Err(
                "Server href points to a different host/port than configured — request blocked"
                    .to_string(),
            );
        }
        Ok(href.to_string())
    } else {
        // Relative path — combine with base_url
        Ok(format!(
            "{}{}",
            base_url.trim_end_matches('/'),
            href
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_ical_events;

    #[test]
    fn parse_ical_events_parses_all_vevents() {
        let ical = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:first\r\nSUMMARY:First\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:second\r\nSUMMARY:Second\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        let events = parse_ical_events(ical);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].uid, "first");
        assert_eq!(events[1].uid, "second");
    }

    #[test]
    fn parse_ical_events_unfolds_folded_lines() {
        let ical = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:u\r\nSUMMARY:Long line\r\nDESCRIPTION:First part\r\n second part\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        let events = parse_ical_events(ical);

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
        assert!(result.is_ok(), "explicit default port should match implicit default");
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
}
