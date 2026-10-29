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

tasks.register("verifyArm64PageAlignment") {
    group = "verification"
    description = "Build the release APK and verify every arm64 native library supports 16 KB pages."
    dependsOn("assembleRelease")

    doLast {
        val apk = project.layout.buildDirectory.file("outputs/apk/release/Glue5-release.apk").get().asFile
        check(apk.isFile) { "Release APK was not produced: $apk" }

        val ndkPath = project.providers.environmentVariable("ANDROID_NDK_HOME").orNull
            ?: error("ANDROID_NDK_HOME is not set; run the Android build through mise")
        val prebuilt = project.file("$ndkPath/toolchains/llvm/prebuilt")
        val executable = if (System.getProperty("os.name").startsWith("Windows", ignoreCase = true)) {
            "llvm-readelf.exe"
        } else {
            "llvm-readelf"
        }
        val readelf = prebuilt.listFiles().orEmpty()
            .asSequence()
            .map { it.resolve("bin/$executable") }
            .firstOrNull { it.isFile }
            ?: error("Could not find $executable under $prebuilt")

        val extracted = temporaryDir.resolve("apk-libs")
        project.delete(extracted)
        project.copy {
            from(project.zipTree(apk))
            include("lib/arm64-v8a/*.so")
            into(extracted)
        }
        val libraries = project.fileTree(extracted) {
            include("lib/arm64-v8a/*.so")
        }.files.sortedBy { it.name }
        check(libraries.isNotEmpty()) { "No arm64 native libraries found in $apk" }

        val failures = mutableListOf<String>()
        for (library in libraries) {
            val process = ProcessBuilder(readelf.absolutePath, "-lW", library.absolutePath)
                .redirectErrorStream(true)
                .start()
            val output = process.inputStream.bufferedReader().use { it.readText() }
            val result = process.waitFor()
            check(result == 0) { "llvm-readelf failed for ${library.name}:\n$output" }

            val loadSegments = output.lineSequence()
                .filter { it.trimStart().startsWith("LOAD ") }
                .toList()
            check(loadSegments.isNotEmpty()) { "No PT_LOAD segments found in ${library.name}" }
            val failureCount = failures.size
            for (segment in loadSegments) {
                val alignmentText = segment.trim().split(Regex("\\s+")).last()
                val alignment = alignmentText.removePrefix("0x").toLongOrNull(16)
                    ?: error("Could not parse PT_LOAD p_align '$alignmentText' in ${library.name}")
                if (alignment < 0x4000) {
                    failures += "${library.name}: PT_LOAD p_align=$alignmentText (< 0x4000)"
                }
            }
            if (failures.size == failureCount) {
                logger.lifecycle("16 KB page alignment OK: ${library.name}")
            }
        }
        check(failures.isEmpty()) {
            "16 KB page-size check failed:\n${failures.joinToString("\n")}"
        }
    }
}
