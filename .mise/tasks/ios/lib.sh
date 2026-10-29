# Shared by the iOS tasks (sourced; not a task itself: not executable).
set -euo pipefail

# Bundle identifiers are unique across all Apple teams: one already taken
# by another team cannot be signed, so GLUE5_BUNDLE_ID can change it (e.g.
# in .mise/config.local.toml); the project specs read it from here.
bundle_id=${GLUE5_BUNDLE_ID:-local.glue5}
export GLUE5_BUNDLE_ID=$bundle_id
# The kind of device the tasks look for (the tvOS tasks set tvOS).
platform=${platform:-iOS}

die() {
    echo "$*" >&2
    exit 1
}

# Builds the app's library (the glue5 crate as a static library) and
# generates the Xcode project from platform/ios/project.yml, next to it.
# The deployment target is as in project.yml; only for iOS builds: in the
# environment of macOS builds it breaks SDL's CMake build.
ios_project() {
    IPHONEOS_DEPLOYMENT_TARGET=16.4 cargo rustc --release -p glue5 --lib \
        --crate-type staticlib --target aarch64-apple-ios
    xcodegen generate --quiet --spec platform/ios/project.yml
}

# xcodebuild DESTINATION [SETTING...]; the app ends up in $app.
app="$PWD/target/ios/derived/Build/Products/Release-iphoneos/Glue 5.app"
xcodebuild_app() {
    local destination=$1
    shift
    xcodebuild -project platform/ios/Glue5.xcodeproj -scheme Glue5 -configuration Release \
        -sdk iphoneos -destination "$destination" -quiet \
        -derivedDataPath target/ios/derived "$@" build
}

# Connected devices of $platform matching a devicectl filter: "identifier
# udid" per line.
devices() {
    xcrun devicectl list devices --hide-default-columns --hide-headers --columns Identifier --columns udid \
        --filter "hardwareProperties.platform == '$platform' AND ($1)" | awk 'length($1) >= 25 { print $1, $2 }'
}

# A reachable device, connected or just paired (identifier, UDID or name; default the only one
# connected): sets $device (its CoreDevice identifier) and $udid.
pick_device() {
    local found
    found=$(devices "State != 'unavailable'")
    if [ -n "${1:-}" ]; then
        found=$(awk -v d="$1" '$1 == d || $2 == d' <<<"$found")
        [ -n "$found" ] || found=$(devices "State != 'unavailable' AND Name == '$1'")
    fi
    case $(grep -c . <<<"$found" || true) in
    1) read -r device udid <<<"$found" ;;
    0) die "no such connected $platform device" ;;
    *) die "several devices connected: pass one ($(awk '{ print $1 }' <<<"$found" | paste -sd, - | sed 's/,/, /g'))" ;;
    esac
}

# Launches the installed app on $device with its console attached. It gets
# the same switches as a desktop run: RUST_LOG and the GLUE5_* variables.
launch() {
    local key
    for key in $(compgen -e | grep '^GLUE5_'); do
        case $key in GLUE5_TEAM | GLUE5_GAME_DIR) continue ;; esac
        export "DEVICECTL_CHILD_$key=${!key}"
    done
    DEVICECTL_CHILD_RUST_LOG=${RUST_LOG:-info} xcrun devicectl device process launch \
        --console --terminate-existing --device "$device" "$bundle_id"
}
