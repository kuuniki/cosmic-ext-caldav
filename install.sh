#!/usr/bin/env bash
set -euo pipefail

REPO_URL="https://github.com/kuuniki/cosmic-ext-caldav.git"
BRANCH="main"
INSTALL_DIR="${HOME}/.local"
BIN_DIR="${INSTALL_DIR}/bin"
APP_DIR="${INSTALL_DIR}/share/applications"
WORK_DIR="$(mktemp -d)"

cleanup() {
    rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "Error: required command not found: $1"
        exit 1
    }
}

echo "==> Checking requirements"
need_cmd git
need_cmd rustc
need_cmd cargo
need_cmd sed
need_cmd install

echo "==> Cloning repository"
git clone --depth 1 --branch "${BRANCH}" --single-branch "${REPO_URL}" "${WORK_DIR}/cosmic-ext-caldav"

cd "${WORK_DIR}/cosmic-ext-caldav"

echo "==> Building release binaries"
if ! cargo build --release --bin cosmic-applet-caldav --bin cosmic-caldav; then
    echo
    echo "Build failed."
    echo "You probably need the required Rust/COSMIC development dependencies installed."
    echo "If you already use COSMIC, make sure your Rust toolchain is installed and up to date."
    exit 1
fi

echo "==> Installing binaries"
mkdir -p "${BIN_DIR}" "${APP_DIR}"
install -Dm755 target/release/cosmic-applet-caldav "${BIN_DIR}/cosmic-applet-caldav"
install -Dm755 target/release/cosmic-caldav "${BIN_DIR}/cosmic-caldav"

echo "==> Installing desktop entries"
install -Dm644 data/cosmic-applet-caldav.desktop "${APP_DIR}/cosmic-applet-caldav.desktop"
install -Dm644 data/cosmic-caldav.desktop "${APP_DIR}/cosmic-caldav.desktop"

echo "==> Updating desktop entry Exec paths"
sed -i "s|^Exec=.*|Exec=${BIN_DIR}/cosmic-applet-caldav|" "${APP_DIR}/cosmic-applet-caldav.desktop"
sed -i "s|^Exec=.*|Exec=${BIN_DIR}/cosmic-caldav|" "${APP_DIR}/cosmic-caldav.desktop"

if ! grep -q '^X-CosmicApplet=true$' "${APP_DIR}/cosmic-applet-caldav.desktop"; then
    printf '\nX-CosmicApplet=true\n' >> "${APP_DIR}/cosmic-applet-caldav.desktop"
fi

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${APP_DIR}" >/dev/null 2>&1 || true
fi

echo
echo "Installation complete."
echo "Opening cosmic-caldav so you can add your account first..."
echo "After that, add the applet by opening:"
echo "COSMIC Settings -> Desktop -> Panel -> Configure panel applets -> Add applet -> CalDAV Calendar"

nohup "${BIN_DIR}/cosmic-caldav" >/dev/null 2>&1 &
