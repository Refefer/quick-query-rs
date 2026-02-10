#!/usr/bin/env bash
#
# Setup AppArmor profile for qq to allow kernel sandbox (user namespaces).
#
# On Ubuntu 24.04+ and other distros with apparmor_restrict_unprivileged_userns=1,
# unprivileged user namespace creation is blocked unless the binary has an AppArmor
# profile granting the "userns" permission. This script creates that profile.
#
# Usage: sudo ./scripts/setup-apparmor.sh [/path/to/qq]

set -euo pipefail

# Check if AppArmor restricts unprivileged user namespaces
RESTRICT_FILE="/proc/sys/kernel/apparmor_restrict_unprivileged_userns"
if [ ! -f "$RESTRICT_FILE" ] || [ "$(cat "$RESTRICT_FILE")" != "1" ]; then
    echo "AppArmor user namespace restriction is not enabled. No profile needed."
    exit 0
fi

# Find the qq binary
QQ_PATH="${1:-$(command -v qq 2>/dev/null || true)}"
if [ -z "$QQ_PATH" ]; then
    echo "Error: qq binary not found. Pass the path as an argument:" >&2
    echo "  sudo $0 /path/to/qq" >&2
    exit 1
fi

# Resolve to absolute path (follow symlinks)
QQ_PATH="$(readlink -f "$QQ_PATH")"
if [ ! -x "$QQ_PATH" ]; then
    echo "Error: $QQ_PATH is not an executable file" >&2
    exit 1
fi

echo "Setting up AppArmor profile for: $QQ_PATH"

# Check for root
if [ "$(id -u)" -ne 0 ]; then
    echo "Error: This script must be run as root (use sudo)" >&2
    exit 1
fi

PROFILE_NAME="qq"
PROFILE_PATH="/etc/apparmor.d/$PROFILE_NAME"

cat > "$PROFILE_PATH" <<EOF
# AppArmor profile for qq (quick-query) - allows user namespace creation
# for the kernel sandbox (hakoniwa). The profile is unconfined otherwise.

abi <abi/4.0>,
include <tunables/global>

profile qq $QQ_PATH flags=(unconfined) {
  userns,

  include if exists <local/qq>
}
EOF

echo "Wrote profile to $PROFILE_PATH"

# Load the profile
apparmor_parser -r "$PROFILE_PATH"
echo "Profile loaded successfully."

# Verify
echo ""
echo "Verifying sandbox support..."
if su -c "'$QQ_PATH' --help" "$(logname 2>/dev/null || echo "${SUDO_USER:-$USER}")" >/dev/null 2>&1; then
    echo "Done. qq kernel sandbox should now be available."
else
    echo "Warning: qq --help failed. The profile is loaded but verify manually."
fi
