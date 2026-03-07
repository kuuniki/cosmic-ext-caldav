# cosmic-ext-caldav
A CalDAV calendar panel applet and settings app for the [COSMIC desktop environment](https://system76.com/cosmic).
## Preview
<img width="364" height="832" alt="image" src="https://github.com/user-attachments/assets/9f422483-028a-4a0a-89fe-b2aa75996d51" />
Supports **Nextcloud**, **Google Calendar**, and **Outlook 365**.
<img width="560" height="505" alt="image" src="https://github.com/user-attachments/assets/f70fe53d-d496-44d4-855e-0f43b16047f6" />
## Features
- Panel applet showing current date/time
- Calendar synced with CalDAV (NextCloud, Google, Outlook); two-way sync
- Create events directly from the panel
- Multiple accounts supported
- Credentials stored locally in `~/.config/cosmic-caldav/config.json`
- Simple UI
## Install from source
```bash
# Arch / CachyOS
sudo pacman -S rust just git
git clone https://github.com/kuuniki/cosmic-ext-caldav
cd cosmic-ext-caldav
cargo build --release
sudo install -Dm0755 target/release/cosmic-caldav /usr/bin/cosmic-caldav
sudo install -Dm0755 target/release/cosmic-applet-caldav /usr/bin/cosmic-applet-caldav
sudo install -Dm0644 data/cosmic-caldav.desktop /usr/share/applications/cosmic-caldav.desktop
sudo install -Dm0644 data/cosmic-applet-caldav.desktop /usr/share/applications/cosmic-applet-caldav.desktop
```
## Setup
### 1. Open the Desktop App
Open **COSMIC CalDAV** from your app launcher. If you can't find it type 'cosmic-caldav' into console.
### 2. Add an account
Click the button to add an account, and then enter your credentials for Nextcloud, Google or Outlook. Instructions are provided in the app. Your password remains hidden from me and is only shared between you and the service provider; in short, it is the same level of security as logging into Google in your browser. I do not see anything.
### 3. Add the applet to your panel
Once an account is set up, go to COSMIC Settings -> Desktop -> Panel -> Configure panel applets -> 'Add applet' -> Select 'CalDAV Calendar' -> Drag it to whichever segment you prefer -> Done. Note: you may want to remove the remove the default 'Date, Time & Calendar' applet to avoid duplication.
## Uninstall
```bash
sudo rm /usr/bin/cosmic-caldav
sudo rm /usr/bin/cosmic-applet-caldav
sudo rm /usr/share/applications/cosmic-caldav.desktop
sudo rm /usr/share/applications/cosmic-applet-caldav.desktop
rm -rf ~/.config/cosmic-caldav
```
## Credits
Built with:
- **[libcosmic](https://github.com/pop-os/libcosmic)** by [System76](https://system76.com) — COSMIC app/applet framework (MPL-2.0)
- **[iced](https://github.com/iced-rs/iced)** — GUI framework underlying libcosmic (MIT)
- **[tokio](https://tokio.rs)** — async runtime (MIT)
- **[reqwest](https://github.com/seanmonstar/reqwest)** — HTTP client for CalDAV requests (MIT/Apache-2.0)
- **[quick-xml](https://github.com/tafia/quick-xml)** — XML parser for CalDAV responses (MIT)
- **[serde](https://serde.rs)** / **[serde_json](https://github.com/serde-rs/json)** — config serialization (MIT/Apache-2.0)
- **[chrono](https://github.com/chronotope/chrono)** — date/time handling (MIT/Apache-2.0)
- **[dirs](https://github.com/dirs-dev/dirs-rs)** — config directory resolution (MIT/Apache-2.0)
- **[once_cell](https://github.com/matklad/once_cell)** — lazy static initialisation (MIT/Apache-2.0)
CalDAV implemented against [RFC 4791](https://www.rfc-editor.org/rfc/rfc4791), iCalendar against [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545).
This project is not affiliated with System76 or the COSMIC project.
## License
MIT
