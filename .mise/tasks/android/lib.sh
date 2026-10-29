# Shared by the Android tasks (sourced; not a task itself: not executable).
set -euo pipefail

package=local.glue5
project=platform/android
# What the build generates for the Gradle project (see its build.gradle.kts).
out=$PWD/target/android
apk=$out/gradle/outputs/apk/release/Glue5-release.apk
abi=arm64-v8a
target=aarch64-linux-android

die() {
    echo "$*" >&2
    exit 1
}

# A setting of the Gradle project (glue5.<name> in its gradle.properties).
setting() {
    sed -n "s/^glue5\.$1=//p" "$project/gradle.properties"
}

# The SDK packages the build uses, in ANDROID_HOME (the android-sdk tool's
# directory): sdkmanager installs the missing ones, asking to accept their
# licences the first time.
sdk_packages() {
    [ -n "${ANDROID_HOME:-}" ] || die "ANDROID_HOME is not set: run the task through mise"
    local packages=("platforms;android-$(setting compileSdk)" "build-tools;$(setting buildTools)"
        "ndk;$(setting ndk)" platform-tools)
    local missing=() package
    for package in "${packages[@]}"; do
        [ -d "$ANDROID_HOME/${package//;//}" ] || missing+=("$package")
    done
    if [ ${#missing[@]} -gt 0 ]; then sdkmanager --install "${missing[@]}"; fi
    export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/$(setting ndk)
    # adb, also when platform-tools came just now.
    PATH=$ANDROID_HOME/platform-tools:$PATH
}

# The app's libraries (the glue5 crate as a shared library and SDL's) and
# the icon's layers, where the Gradle project takes them.
android_libs() {
    # SDL's CMake build (cmake crate) with the NDK's toolchain file: cargo-ndk
    # sets up the compilers for cc and rustc, not for CMake.
    export CMAKE_TOOLCHAIN_FILE_${target//-/_}=$ANDROID_NDK_HOME/build/cmake/android.toolchain.cmake
    cargo ndk --target "$abi" --platform "$(setting minSdk)" \
        rustc --release -p glue5 --lib --crate-type cdylib
    rm -rf "$out/jniLibs" "$out/res"
    mkdir -p "$out/jniLibs/$abi" "$out/res/drawable-nodpi"
    # sdl3-sys puts libSDL3.so next to the crate's library, glue5-gpu
    # libadrenotools' (GPU drivers users add); Luau (the mods' language) is
    # C++, built against the NDK's shared libc++.
    local lib
    for lib in glue5 SDL3 adrenotools hook_impl main_hook; do
        ln -f "target/$target/release/lib$lib.so" "$out/jniLibs/$abi/"
    done
    cp "$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/*/sysroot/usr/lib/"$target"/libc++_shared.so "$out/jniLibs/$abi/"
    cp platform/shared/Glue5.icon/Assets/01-background.png "$out/res/drawable-nodpi/glue5_background.png"
    cp platform/shared/Glue5.icon/Assets/02-five.png "$out/res/drawable-nodpi/glue5_five.png"
}

# The APK, signed with the debug key; it ends up in $apk.
android_apk() {
    # SDL's Java side, from the sdl3-src crate the libraries were built with.
    local sdl
    # On Windows the paths come with escaped backslashes (C:\\...): made
    # forward slashes, which Gradle takes too.
    sdl=$(cargo metadata --format-version 1 --locked |
        grep -o '"manifest_path":"[^"]*sdl3-src-[^"/\\]*[/\\]*Cargo.toml"' | head -1 | cut -d'"' -f4 |
        sed 's|\\\\|/|g')
    [ -n "$sdl" ] || die "sdl3-src is not among the dependencies"
    gradle -p "$project" --project-cache-dir "$out/gradle-cache" --quiet \
        "-Pglue5.sdlJava=${sdl%/Cargo.toml}/SDL/android-project/app/src/main/java" assembleRelease
}

# Launches the installed app and follows its log until it ends; then shows
# a crash's report, if any. It gets the same switches as a desktop run:
# RUST_LOG and the GLUE5_* variables, as the launch intent's extras.
launch() {
    local since pid logcat key extras=()
    for key in RUST_LOG $(compgen -e | grep '^GLUE5_'); do
        case $key in GLUE5_TEAM | GLUE5_GAME_DIR) continue ;; esac
        [ -n "${!key:-}" ] || continue
        # adb shell joins its arguments into one command line: quoted for it.
        extras+=(--es "$key" "'${!key//\'/\'\\\'\'}'")
    done
    since=$(adb shell date +%s.000)
    adb shell am start -W -S -n "$package/.Glue5Activity" ${extras[@]+"${extras[@]}"} >/dev/null
    pid=$(adb shell pidof -s "$package") || die "the app did not start"
    adb logcat -v brief --pid="$pid" &
    logcat=$!
    trap 'kill ${logcat:-} 2>/dev/null' EXIT
    # Waits on the device (polling through adb would log a line per call).
    adb shell "while [ -d /proc/$pid ]; do sleep 1; done"
    sleep 1
    adb logcat -d -b crash -v brief -T "$since"
}
