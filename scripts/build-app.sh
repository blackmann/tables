#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mode="${TABLES_BUILD_MODE:-release}"
case "$mode" in
  debug) cargo build --locked ;;
  release) cargo build --release --locked ;;
  *) printf "Unknown build mode: %s\n" "$mode" >&2; exit 1 ;;
esac
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
app="build/Tables.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$mode/tables" "$app/Contents/MacOS/tables"
icon_key=""
if [ -f assets/AppIcon.icns ]; then
  cp assets/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
  icon_key="<key>CFBundleIconFile</key><string>AppIcon</string>"
fi
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>tables</string>
<key>CFBundleIdentifier</key><string>uk.co.degreat.Tables</string>
<key>CFBundleName</key><string>Tables</string>
<key>CFBundleDisplayName</key><string>Tables</string>
$icon_key
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
# macOS remembers privacy grants and Keychain access by the
# app's signature. An ad-hoc signature is just a hash of this build, so every
# update would look like a new app and ask again. A real identity keeps the
# grants across builds. In order: TABLES_SIGN_IDENTITY if set, else the identity
# saved by `install-app.sh --choose-identity` (if still in the keychain), else
# the first Developer ID or Apple Development certificate.
saved_identity_file=".signing-identity"
identity="${TABLES_SIGN_IDENTITY:-}"
identities=$(security find-identity -v -p codesigning 2>/dev/null || true)
if [ -z "$identity" ] && [ -f "$saved_identity_file" ]; then
  saved=$(tr -d '[:space:]' < "$saved_identity_file")
  if [ -n "$saved" ] && printf '%s\n' "$identities" | grep -qF "$saved"; then
    identity="$saved"
  else
    printf 'Saved signing identity %s is no longer in the keychain; run install-app.sh --choose-identity to pick another.\n' "$saved" >&2
  fi
fi
if [ -z "$identity" ]; then
  for kind in "Developer ID Application" "Apple Development"; do
    identity=$(printf '%s\n' "$identities" | sed -n "s/.*\"\($kind: .*\)\"/\1/p" | head -n 1)
    [ -n "$identity" ] && break
  done
fi
if [ -z "$identity" ]; then
  identity="-"
  printf 'No signing identity found: signing ad hoc, so macOS will ask for permissions again after each update.\n' >&2
fi
codesign --force --deep --sign "$identity" "$app"
printf 'Built %s (signed by %s)\n' "$app" "$identity"
