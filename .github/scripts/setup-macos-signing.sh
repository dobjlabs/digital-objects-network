#!/usr/bin/env bash
set -euo pipefail

missing=""
for name in APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY_SECRET KEYCHAIN_PASSWORD APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_CONTENT; do
  if [[ -z "${!name:-}" ]]; then
    missing+=" $name"
  fi
done
if [[ -n "$missing" ]]; then
  echo "::error::Missing required macOS signing/notarization secrets:$missing"
  exit 1
fi

umask 077
certificate_path="$RUNNER_TEMP/apple-certificate.p12"
keychain_path="$RUNNER_TEMP/build.keychain-db"
api_key_path="$RUNNER_TEMP/AuthKey_$APPLE_API_KEY.p8"
printf '%s' "$APPLE_CERTIFICATE" | base64 --decode > "$certificate_path"
security create-keychain -p "$KEYCHAIN_PASSWORD" "$keychain_path"
security default-keychain -s "$keychain_path"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$keychain_path"
security set-keychain-settings -t 3600 -u "$keychain_path"
security import "$certificate_path" -k "$keychain_path" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$keychain_path"
printf '%s' "$APPLE_API_KEY_CONTENT" > "$api_key_path"

{
  echo "MACOS_SIGNING_IDENTITY=$APPLE_SIGNING_IDENTITY_SECRET"
  echo "APPLE_SIGNING_IDENTITY=$APPLE_SIGNING_IDENTITY_SECRET"
  echo "APPLE_API_KEY=$APPLE_API_KEY"
  echo "APPLE_API_KEY_ID=$APPLE_API_KEY"
  echo "APPLE_API_ISSUER=$APPLE_API_ISSUER"
  echo "APPLE_API_KEY_PATH=$api_key_path"
} >> "$GITHUB_ENV"
