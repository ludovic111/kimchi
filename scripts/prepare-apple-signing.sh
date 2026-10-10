#!/usr/bin/env bash
# Release workflow, macOS runners: import the Developer ID certificate into a throwaway keychain,
# check that it holds the signing identity, and write the App Store Connect API key, then hand
# scripts/bundle-macos.sh the identity and the notarization key through $GITHUB_ENV. Same secrets as ryolune
# (see ../lsuite/STANDARD.md). The certificate password may be empty (a .p12 exported without one).
set -euo pipefail
for name in APPLE_CERTIFICATE_P12_BASE64 APPLE_SIGNING_IDENTITY APPLE_API_KEY_P8_BASE64 APPLE_API_KEY_ID APPLE_API_ISSUER; do
  if [ -z "${!name:-}" ]; then
    echo "Missing secret $name" >&2
    exit 1
  fi
done
keychain="$RUNNER_TEMP/signing.keychain-db"
# The Mac mini's runner is its owner's account: save the keychain search list first, so the
# workflows' cleanup step puts it back exactly as it was (one path per line).
security list-keychains -d user | sed -e 's/^ *"//' -e 's/"$//' > "$RUNNER_TEMP/keychains-before"
password=$(uuidgen)
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
umask 077
printf '%s' "$APPLE_CERTIFICATE_P12_BASE64" | base64 --decode > "$RUNNER_TEMP/developer-id.p12"
security import "$RUNNER_TEMP/developer-id.p12" -k "$keychain" -P "${APPLE_CERTIFICATE_PASSWORD:-}" \
  -f pkcs12 -T /usr/bin/codesign
rm -f "$RUNNER_TEMP/developer-id.p12"
security set-key-partition-list -S apple-tool:,apple: -s -k "$password" "$keychain" > /dev/null
# Search the new keychain first, keeping the runner's own keychains after it.
existing=()
while IFS= read -r k; do [ -n "$k" ] && [ "$k" != "$keychain" ] && existing+=("$k"); done < "$RUNNER_TEMP/keychains-before"
security list-keychains -d user -s "$keychain" ${existing[@]+"${existing[@]}"}
if ! security find-identity -v -p codesigning "$keychain" | grep -qF "$APPLE_SIGNING_IDENTITY"; then
  echo "The certificate does not hold a valid identity \"$APPLE_SIGNING_IDENTITY\" (was the private key exported with it?)." >&2
  security find-identity -v -p codesigning "$keychain" >&2
  exit 1
fi
printf '%s' "$APPLE_API_KEY_P8_BASE64" | base64 --decode > "$RUNNER_TEMP/notary.p8"
{
  echo "APPLE_SIGNING_IDENTITY=$APPLE_SIGNING_IDENTITY"
  echo "APPLE_API_KEY=$APPLE_API_KEY_ID"
  echo "APPLE_API_ISSUER=$APPLE_API_ISSUER"
  echo "APPLE_API_KEY_PATH=$RUNNER_TEMP/notary.p8"
} >> "$GITHUB_ENV"
echo "Developer ID signing and notarization are ready."
