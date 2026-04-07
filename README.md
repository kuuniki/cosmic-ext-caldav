## Overview

An integrated panel calendar + CalDAV applet for the COSMIC Desktop Environment. This applet is for COSMIC desktop users who want convenient CalDAV calendar access (such as from providers like Google, Nextcloud, Outlook/365, or other CalDAV-compatible services) directly via the panel. This applet is lightweight and provides an integrated desktop experience without extra clutter on your panel. 

**Key Features**
  - Add and manage multiple CalDAV accounts
  - Full integration to the COSMIC panel (make, view, edit, delete calendar events directly from the panel)
  - Time and date view in the panel to replace the core calendar (also shows day of the week!)
  - Login details stored securely on your system using the system keyring, I do not get access to any of your credentials

## Install

**Step 1:** Paste this into a terminal/console:

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/kuuniki/cosmic-ext-caldav/main/install.sh)
```

**Step 2:** Add your account


<img width="560" height="505" alt="image" src="https://github.com/user-attachments/assets/21ebf13b-efb5-489c-8c36-3709d975aa72" />

  
**Step 3:** After that, add the applet by opening:


COSMIC Settings -> Desktop -> Panel -> Configure panel applets -> Add applet -> CalDAV Calendar
<img width="1024" height="768" alt="image" src="https://github.com/user-attachments/assets/0fc3bf51-0973-458d-aee2-b8c1a473f0a4" />


## Acknowledgements

This project was built with the assistance of AI tools (vibecoded); specifically, Claude and Codex.

**Dependencies**
- **`libcosmic`** = UI + COSMIC integration
- **`reqwest` + `quick-xml`** = talk to CalDAV servers
- **`tokio` + `futures-util`** = do network/background work without freezing
- **`chrono` + `chrono-tz`** = dates, times, timezones
- **`serde` + `serde_json` + `dirs`** = config storage
- **`keyring` + `zeroize`** = safer secret handling
- **`uuid`** = unique event IDs
