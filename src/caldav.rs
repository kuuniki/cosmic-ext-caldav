use chrono::{DateTime, Utc};
use reqwest::{Client, header};
use quick_xml::events::Event;
use quick_xml::Reader;

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
        Self { base_url, username, password, client }
    }

    fn caldav_url(&self) -> String {
        format!("{}/remote.php/dav/calendars/{}/", self.base_url, self.username)
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

        let resp = self.client
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "1")
            .header(header::CONTENT_TYPE, "application/xml")
            .body(body)
            .send()
            .await
            .map_err(|e| format!("Request error: {}", e))?;

        let text = resp.text().await.map_err(|e| format!("Read error: {}", e))?;
        parse_calendars(&text)
    }

    pub async fn get_events(&self, calendar_href: &str) -> Result<Vec<CalendarEvent>, String> {
        let url = if calendar_href.starts_with("http") {
            calendar_href.to_string()
        } else {
            format!("{}{}", self.base_url, calendar_href)
        };

        let body = r#"<?xml version="1.0"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT"/>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#;

        let resp = self.client
            .request(reqwest::Method::from_bytes(b"REPORT").unwrap(), &url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "1")
            .header(header::CONTENT_TYPE, "application/xml")
            .body(body)
            .send()
            .await
            .map_err(|e| format!("Request error: {}", e))?;

        let text = resp.text().await.map_err(|e| format!("Read error: {}", e))?;
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
                    "response" => { in_response = true; current_href.clear(); current_name.clear(); current_color = None; }
                    "href" if in_response => in_href = true,
                    "displayname" => in_displayname = true,
                    "calendar-color" => in_color = true,
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if in_href { current_href = text.clone(); in_href = false; }
                if in_displayname { current_name = text.clone(); in_displayname = false; }
                if in_color { current_color = Some(text); in_color = false; }
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
            Err(_) => break,
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
                if name == "calendar-data" { in_calendar_data = true; calendar_data.clear(); }
            }
            Ok(Event::Text(e)) => {
                if in_calendar_data { calendar_data.push_str(&e.unescape().unwrap_or_default()); }
            }
            Ok(Event::CData(e)) => {
                if in_calendar_data { calendar_data.push_str(&String::from_utf8_lossy(&e)); }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if name == "calendar-data" && in_calendar_data {
                    in_calendar_data = false;
                    if let Some(event) = parse_ical_event(&calendar_data) {
                        events.push(event);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    Ok(events)
}

fn parse_ical_event(ical: &str) -> Option<CalendarEvent> {
    let mut uid = String::new();
    let mut summary = String::new();
    let mut dtstart: Option<DateTime<Utc>> = None;
    let mut dtend: Option<DateTime<Utc>> = None;
    let mut description = None;
    let mut location = None;
    let mut in_vevent = false;

    for line in ical.lines() {
        let line = line.trim();
        if line.starts_with("BEGIN:VEVENT") { in_vevent = true; continue; }
        if line.starts_with("END:VEVENT") { in_vevent = false; continue; }
        if !in_vevent { continue; }
        if let Some(val) = line.strip_prefix("UID:") {
            uid = val.to_string();
        } else if let Some(val) = line.strip_prefix("SUMMARY:") {
            summary = val.replace("\\n", "\n").replace("\\,", ",");
        } else if line.starts_with("DTSTART") {
            dtstart = parse_ical_date(line);
        } else if line.starts_with("DTEND") {
            dtend = parse_ical_date(line);
        } else if let Some(val) = line.strip_prefix("DESCRIPTION:") {
            description = Some(val.replace("\\n", "\n").replace("\\,", ","));
        } else if let Some(val) = line.strip_prefix("LOCATION:") {
            location = Some(val.replace("\\n", "\n").replace("\\,", ","));
        }
    }

    if uid.is_empty() && summary.is_empty() { return None; }
    Some(CalendarEvent { uid, summary, start: dtstart, end: dtend, description, location })
}

fn parse_ical_date(line: &str) -> Option<DateTime<Utc>> {
    let val = line.split(':').last()?.trim();
    if val.ends_with('Z') && val.len() >= 15 {
        return chrono::NaiveDateTime::parse_from_str(&val[..15], "%Y%m%dT%H%M%S")
            .ok().map(|ndt| ndt.and_utc());
    }
    if val.len() == 8 {
        return chrono::NaiveDate::parse_from_str(val, "%Y%m%d")
            .ok().map(|d| d.and_hms_opt(0,0,0).unwrap().and_utc());
    }
    if val.len() >= 15 {
        return chrono::NaiveDateTime::parse_from_str(&val[..15], "%Y%m%dT%H%M%S")
            .ok().map(|ndt| ndt.and_utc());
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
        let start = chrono::Local.from_local_datetime(
            &date.and_hms_opt(hour, minute, 0).unwrap()
        ).unwrap();
        let end = start + chrono::Duration::minutes(duration_mins as i64);
        let fmt = "%Y%m%dT%H%M%S";
        let alarm = if reminder_mins > 0 {
            format!("BEGIN:VALARM\r\nTRIGGER:-PT{}M\r\nACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\n", reminder_mins)
        } else {
            String::new()
        };
        let loc_line = if !location.is_empty() { format!("LOCATION:{}\r\n", location) } else { String::new() };
        let desc_line = if !description.is_empty() { format!("DESCRIPTION:{}\r\n", description) } else { String::new() };
        let ical = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//cosmic-caldav//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{summary}\r\n{loc}{desc}DTSTART:{start}\r\nDTEND:{end}\r\n{alarm}END:VEVENT\r\nEND:VCALENDAR\r\n",
            uid = uid,
            summary = summary,
            loc = loc_line,
            desc = desc_line,
            start = start.format(fmt),
            end = end.format(fmt),
            alarm = alarm,
        );
        let url = format!("{}{}{}.ics", self.base_url, calendar_href, uid);
        let resp = self.client
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
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{:x}-{:x}", t.as_secs(), t.subsec_nanos())
}
