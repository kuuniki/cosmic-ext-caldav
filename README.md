# cosmic-ext-caldav

CalDAV integration for COSMIC, with two binaries:
- `cosmic-caldav`: settings/configuration app
- `cosmic-applet-caldav`: panel applet with calendar/events view and quick event creation

## Install (from source)

### 1) Install system dependencies
On Debian/Ubuntu-based systems:

```bash
sudo apt update
sudo apt install -y \
  build-essential \
  pkg-config \
  libxkbcommon-dev \
  libssl-dev
```

> `libxkbcommon-dev` is required by Wayland dependencies used by `libcosmic`.

### 2) Build

```bash
cargo build --release
```

### 3) Install binaries and desktop entries
From repo root:

```bash
sudo install -Dm755 target/release/cosmic-caldav /usr/local/bin/cosmic-caldav
sudo install -Dm755 target/release/cosmic-applet-caldav /usr/local/bin/cosmic-applet-caldav

sudo install -Dm644 data/cosmic-caldav.desktop /usr/share/applications/cosmic-caldav.desktop
sudo install -Dm644 data/cosmic-applet-caldav.desktop /usr/share/applications/cosmic-applet-caldav.desktop
```

## Using start/end date+time event creation

The start/end date+time form is in the **applet popup** (`cosmic-applet-caldav`), not the settings app.

1. Open the applet popup from your panel.
2. Click **Add event** in the popup header.
3. Fill:
   - `Start (YYYY-MM-DD, HH:MM)`
   - `End (YYYY-MM-DD, HH:MM)`
4. Save.

This supports multi-day events naturally (for example, start on one day and end on a later day).
