#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
source_app="$repo_root/build/Tables.app"
install_dir="${TABLES_INSTALL_DIR:-/Applications}"
destination="$install_dir/Tables.app"

usage() {
  cat <<USAGE
Usage: $(basename "$0") [--choose-identity]

Builds Tables.app, signs it and installs it to \$TABLES_INSTALL_DIR (default /Applications).

  --choose-identity  List the code-signing identities in your keychain and pick
                     the one to sign with. The choice is saved to
                     .signing-identity and reused by later builds.
  -h, --help         Show this help.

\$TABLES_SIGN_IDENTITY, if set, overrides the saved choice. With neither, the build
signs with the first Developer ID or Apple Development certificate it finds.
USAGE
}

choose_identity=false
for arg in "$@"; do
  case "$arg" in
    --choose-identity) choose_identity=true ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'Unknown option: %s\n\n' "$arg" >&2; usage >&2; exit 1 ;;
  esac
done

if [ "$(uname -s)" != "Darwin" ]; then
  printf 'Tables can only be installed as an app on macOS.\n' >&2
  exit 1
fi

# Prints the team (OU) and organisation (O) of the certificate with SHA-1 $1,
# since two identities can share a name across teams.
certificate_team() {
  security find-certificate -a -Z -p 2>/dev/null \
    | awk -v h="$1" '/^SHA-1 hash:/ { p = ($3 == h); next } p { print } p && /-----END/ { exit }' \
    | openssl x509 -noout -subject -nameopt sep_multiline,utf8 2>/dev/null \
    | awk -F= '/^ *OU=/ { ou = $2 } /^ *O=/ { o = $2 } END { if (ou != "") printf "team %s%s", ou, (o != "" ? ", " o : "") }'
}

pick_identity() {
  list=$(security find-identity -v -p codesigning 2>/dev/null \
    | sed -n 's/^ *[0-9][0-9]*) \([0-9A-F]\{40\}\) "\(.*\)"$/\1 \2/p')
  if [ -z "$list" ]; then
    printf 'No code-signing identities found in your keychain; the app will be signed ad hoc.\n' >&2
    return
  fi
  current=""
  [ -f "$repo_root/.signing-identity" ] && current=$(tr -d '[:space:]' < "$repo_root/.signing-identity")
  printf 'Code-signing identities:\n'
  count=0
  while IFS= read -r line; do
    count=$((count + 1))
    hash=${line%% *}
    name=${line#* }
    team=$(certificate_team "$hash")
    marker=""
    [ "$hash" = "$current" ] && marker=" (current)"
    printf '  %d) %s%s\n     %s%s\n' "$count" "$name" "$marker" "$hash" "${team:+  [$team]}"
  done <<EOF_LIST
$list
EOF_LIST
  while :; do
    printf 'Sign with [1-%d]: ' "$count"
    read -r choice < /dev/tty || exit 1
    case "$choice" in
      ''|*[!0-9]*) ;;
      *) [ "$choice" -ge 1 ] && [ "$choice" -le "$count" ] && break ;;
    esac
    printf 'Enter a number between 1 and %d.\n' "$count"
  done
  chosen=$(printf '%s\n' "$list" | sed -n "${choice}p")
  printf '%s\n' "${chosen%% *}" > "$repo_root/.signing-identity"
  printf 'Saved %s; later builds will sign with it.\n' "${chosen#* }"
}

if $choose_identity; then
  pick_identity
fi

"$repo_root/scripts/build-app.sh"

if [ ! -d "$install_dir" ]; then
  if ! mkdir -p "$install_dir" 2>/dev/null; then
    printf 'Creating %s requires administrator access.\n' "$install_dir"
    sudo mkdir -p "$install_dir"
  fi
fi

if [ -w "$install_dir" ] && { [ ! -e "$destination" ] || [ -w "$destination" ]; }; then
  ditto "$source_app" "$destination"
else
  printf 'Installing to %s requires administrator access.\n' "$install_dir"
  sudo ditto "$source_app" "$destination"
fi

codesign --verify --deep --strict "$destination"
# ditto leaves the bundle directory's mtime alone, and macOS only rereads an
# app's icon when that changes, so a new icon would keep showing the old one.
touch "$destination" 2>/dev/null || sudo touch "$destination"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$destination"
printf 'Installed %s\n' "$destination"
