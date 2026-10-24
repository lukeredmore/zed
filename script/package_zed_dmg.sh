#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

BUILD_NUMBER="${ZED_BUILD_NUMBER:-0}"
OUTPUT_DMG="${REPO_ROOT}/target/release/Zed.dmg"
ZED_BINARY=""
CLI_BINARY=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --build-number)
      BUILD_NUMBER="$2"
      shift 2
      ;;
    --output-dmg)
      OUTPUT_DMG="$2"
      shift 2
      ;;
    --zed-binary)
      ZED_BINARY="$2"
      shift 2
      ;;
    --cli-binary)
      CLI_BINARY="$2"
      shift 2
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

UPSTREAM_VERSION="$(python3 -c '
import pathlib, re
text = pathlib.Path("crates/zed/Cargo.toml").read_text()
m = re.search(r"^version\s*=\s*\"([^\"]+)\"", text, re.M)
print(m.group(1) if m else "0.241.0")
')"
GIT_SHA="$(git rev-parse HEAD)"
SHORT_SHA="$(git rev-parse --short=7 HEAD)"
FULL_VERSION="${UPSTREAM_VERSION}+stable.${BUILD_NUMBER}.${SHORT_SHA}"

if [[ -z "${ZED_BINARY}" ]]; then
  export ZED_RELEASE_CHANNEL="stable"
  export ZED_BUNDLE="true"
  export ZED_COMMIT_SHA="${GIT_SHA}"
  if [[ ! -f assets/licenses.md ]]; then
    mkdir -p assets
    touch assets/licenses.md
  fi
  cargo build --release --package zed --bin zed --package cli --bin cli
  ZED_BINARY="${REPO_ROOT}/target/release/zed"
  CLI_BINARY="${REPO_ROOT}/target/release/cli"
fi

STAGE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/zed-dmg-stage.XXXXXX")"
trap 'rm -rf "${STAGE_DIR}"' EXIT

APP_DIR="${STAGE_DIR}/Zed.app"
mkdir -p "${APP_DIR}/Contents/MacOS" "${APP_DIR}/Contents/Resources"

cp "${ZED_BINARY}" "${APP_DIR}/Contents/MacOS/zed"
chmod +x "${APP_DIR}/Contents/MacOS/zed"
if [[ -n "${CLI_BINARY}" && -f "${CLI_BINARY}" ]]; then
  cp "${CLI_BINARY}" "${APP_DIR}/Contents/MacOS/cli"
  chmod +x "${APP_DIR}/Contents/MacOS/cli"
fi

cp "${REPO_ROOT}/crates/zed/resources/AppIcon.icns" "${APP_DIR}/Contents/Resources/AppIcon.icns"
cp "${REPO_ROOT}/crates/zed/resources/Document.icns" "${APP_DIR}/Contents/Resources/Document.icns"
cp "${REPO_ROOT}/crates/zed/resources/app-icon.png" "${APP_DIR}/Contents/Resources/app-icon.png"
cp "${REPO_ROOT}/crates/zed/resources/app-icon@2x.png" "${APP_DIR}/Contents/Resources/app-icon@2x.png"

cat > "${APP_DIR}/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>Zed</string>
  <key>CFBundleExecutable</key>
  <string>zed</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleIconName</key>
  <string>AppIcon</string>
  <key>CFBundleIdentifier</key>
  <string>dev.zed.Zed</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Zed</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${UPSTREAM_VERSION}</string>
  <key>CFBundleVersion</key>
  <string>${BUILD_NUMBER}</string>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key>
  <true/>
  <key>ZedReleaseChannel</key>
  <string>stable</string>
  <key>ZedUpstreamVersion</key>
  <string>${UPSTREAM_VERSION}</string>
  <key>ZedFullVersion</key>
  <string>${FULL_VERSION}</string>
  <key>ZedCommitSHA</key>
  <string>${SHORT_SHA}</string>
  <key>ZedGitCommit</key>
  <string>${GIT_SHA}</string>
</dict>
</plist>
EOF

codesign --force --deep --sign - "${APP_DIR}"

ln -s /Applications "${STAGE_DIR}/Applications"
mkdir -p "$(dirname "${OUTPUT_DMG}")"
rm -f "${OUTPUT_DMG}"

hdiutil create \
  -volname "Zed" \
  -srcfolder "${STAGE_DIR}" \
  -ov \
  -format UDZO \
  "${OUTPUT_DMG}"

echo "Packaged ${OUTPUT_DMG} (version=${FULL_VERSION}, sha=${GIT_SHA})"
