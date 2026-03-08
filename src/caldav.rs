use chrono::{DateTime, Utc};
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::{header, Client};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CalendarEvent {
    pub uid: String,
    pub summary: String,
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    pub description: Option<String>,
    pub location: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Calendar {
    pub href: String,
    pub display_name: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CalDavClient {
    pub base_url: String,
    pub username: String,
    pub password: String,
    client: Client,
}

impl CalDavClient {
    pub fn new(base_url: String, username: String, password: String) -> Self {
        let client = Client::new();
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
            format!(
                "{}/remote.php/dav/calendars/{}/",
                self.base_url, self.username
            )
        }
    }

    pub async fn test_connection(&self) -> Result<(), String> {
        let url = self.caldav_url();
        let resp = self.client
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "0")
            .header(header::CONTENT_TYPE, "application/xml")
            .body(r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:displayname/></d:prop></d:propfind>"#)
            .send()
            .await
            .map_err(|e| format!("Connection error: {}", e))?;

        if resp.status().is_success() || resp.status().as_u16() == 207 {
            Ok(())
        } else {
            Err(format!("Auth failed: HTTP {}", resp.status()))
        }
    }

    pub async fn get_calendars(&self) -> Result<Vec<Calendar>, String> {
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
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "1")
            .header(header::CONTENT_TYPE, "application/xml")
            .body(body)
            .send()
            .await
            .map_err(|e| format!("Request error: {}", e))?;

        let text = resp
            .text()
            .await
            .map_err(|e| format!("Read error: {}", e))?;
        parse_calendars(&text)
    }

    pub async fn get_events(&self, calendar_href: &str) -> Result<Vec<CalendarEvent>, String> {
        let url = if calendar_href.starts_with("http") {
            calendar_href.to_string()
        } else {
            format!("{}{}", self.base_url, calendar_href)
        };

        let now = chrono::Utc::now();
        let start = (now - chrono::Duration::days(60)).format("%Y%m%dT000000Z");
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
            .request(reqwest::Method::from_bytes(b"REPORT").unwrap(), &url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "1")
            .header(header::CONTENT_TYPE, "application/xml")
            .body(body)
            .send()
            .await
            .map_err(|e| format!("Request error: {}", e))?;

        let text = resp
            .text()
            .await
            .map_err(|e| format!("Read error: {}", e))?;
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
                if name == "response" && in_response {
                    if current_href.ends_with('/') && !current_name.is_empty() {
                        calendars.push(Calendar {
                            href: current_href.clone(),
                            display_name: current_name.clone(),
                            color: current_color.clone(),
                        });
                    }
                    in_response = false;
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
        use chrono::TimeZone;
        let uid = format!("{}", uuid_simple());
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
        let url = if calendar_href.starts_with("http") {
            format!("{}/{}.ics", calendar_href.trim_end_matches('/'), uid)
        } else {
            format!("{}/{}/{}.ics", self.base_url.trim_end_matches('/'), calendar_href.trim_matches('/'), uid)
        };
        let resp = self
            .client
            .put(&url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Content-Type", "text/calendar; charset=utf-8")
            .body(ical)
            .send()
            .await
            .map_err(|e| format!("Request error: {e}"))?;
        if resp.status().is_success() || resp.status().as_u16() == 201 {
            Ok(())
        } else {
            Err(format!("Server error: HTTP {}", resp.status()))
        }
    }
}


fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:x}-{:x}", t.as_secs(), t.subsec_nanos())
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
}
