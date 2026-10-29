// The Android app: a thin package around the glue5 library (the same crate
// as the desktop and iOS apps) and SDL's Java side. `mise run android` /
// `android:build` build the libraries and the icon's drawables into
// target/android, then run this with glue5.sdlJava: SDL's Java sources, from
// the sdl3-src crate the libraries were built with. Gradle's output goes to
// target/android/gradle.

plugins {
    id("com.android.application") version "9.4.1"
}

val repo = rootDir.resolve("../..")
val generated = repo.resolve("target/android")
val cargoVersion = Regex("""(?m)^version = "([^"]+)"$""")
    .find(repo.resolve("Cargo.toml").readText())?.groupValues?.get(1)
    ?: error("Could not read the workspace version from Cargo.toml")
val versionCodename = repo.resolve("VERSION_CODENAME").readText().trim()
require(versionCodename.matches(Regex("[a-z]+"))) { "VERSION_CODENAME must be one lowercase word" }
// Bump by hand for a new distributable build; this does not change SemVer.
val appVersionCode = repo.resolve("VERSION_CODE").takeIf { it.isFile }?.readText()?.trim()?.toInt() ?: 1
require(appVersionCode > 0) { "VERSION_CODE must be a positive integer" }
fun setting(name: String): String = providers.gradleProperty("glue5.$name").get()

layout.buildDirectory = generated.resolve("gradle")

android {
    namespace = "local.glue5"
    compileSdk = setting("compileSdk").toInt()
    buildToolsVersion = setting("buildTools")
    // For stripping the libraries' debug information.
    ndkVersion = setting("ndk")

    buildFeatures {
        aidl = true
    }

    defaultConfig {
        applicationId = "local.glue5"
        minSdk = setting("minSdk").toInt()
        targetSdk = setting("targetSdk").toInt()
        versionCode = appVersionCode
        versionName = "$cargoVersion $versionCodename"
        ndk { abiFilters += "arm64-v8a" }
    }

    buildTypes {
        release {
            // For development devices, as the iOS app: the debug key.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    sourceSets["main"].apply {
        manifest.srcFile("AndroidManifest.xml")
        java.srcDirs("java", setting("sdlJava"))
        aidl.srcDirs("aidl")
        res.srcDirs("res", generated.resolve("res"))
        jniLibs.srcDirs(generated.resolve("jniLibs"))
    }

    lint {
        // Not on every release build (it is slow): `gradle lint` by hand.
        checkReleaseBuilds = false
    }
}

dependencies {
    // Official Shizuku API: the user starts Shizuku via Wireless debugging
    // and explicitly grants GLUE 5 permission before we read logcat.
    implementation("dev.rikka.shizuku:api:13.1.5")
    implementation("dev.rikka.shizuku:provider:13.1.5")
    // Share a complete capture file when it is too large for Android's clipboard.
    implementation("androidx.core:core:1.15.0")
}
