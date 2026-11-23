import java.util.Base64
import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
    id("com.google.devtools.ksp")
}

val localProperties = Properties().apply {
    val file = rootProject.file("local.properties")
    if (file.exists()) {
        file.inputStream().use { load(it) }
    }
}

fun setting(name: String, fallback: String = ""): String =
    (localProperties.getProperty(name) ?: System.getenv(name) ?: fallback).trim()

// Release signing comes only from the environment (CI secrets or your shell); nothing is committed.
// Without it, `assembleRelease` produces an UNSIGNED apk that must not be distributed.
val releaseKeystore: File? =
    System.getenv("DUOCHAT_KEYSTORE_PATH")?.let { file(it) }
        ?: System.getenv("DUOCHAT_KEYSTORE_B64")?.takeIf { it.isNotBlank() }?.let { encoded ->
            layout.buildDirectory.file("release.keystore").get().asFile.also {
                it.parentFile.mkdirs()
                it.writeBytes(Base64.getMimeDecoder().decode(encoded))
            }
        }

fun com.android.build.api.dsl.ApplicationDefaultConfig.buildConfigString(name: String, value: String) {
    val escaped = value.replace("\\", "\\\\").replace("\"", "\\\"")
    buildConfigField("String", name, "\"$escaped\"")
}

android {
    namespace = "com.duochat.app"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.duochat.app"
        minSdk = 26
        // The TURN relay built into this release (empty: none). Set in CI from repository secrets; see docs/turn-server/README.md.
        buildConfigString("PRIVCORD_TURN_URLS", System.getenv("PRIVCORD_TURN_URLS") ?: "")
        buildConfigString("PRIVCORD_TURN_USERNAME", System.getenv("PRIVCORD_TURN_USERNAME") ?: "")
        buildConfigString("PRIVCORD_TURN_CREDENTIAL", System.getenv("PRIVCORD_TURN_CREDENTIAL") ?: "")
        targetSdk = 35
        // The automatic builds pass their run number, so every build is newer than the last (updates need that).
        versionCode = (project.findProperty("duochat.versionCode") as String?)?.toIntOrNull() ?: 1
        versionName = (project.findProperty("duochat.versionName") as String?) ?: "0.1.0"

        // Only the languages the app speaks: drops the libraries' translations for every other language.
        resourceConfigurations += listOf("en", "es", "fr")

        buildConfigString("CONVERSATION_ID", setting("duochat.conversationId", "duo"))
        buildConfigString("TURN_URL", setting("turn.url"))
        buildConfigString("TURN_USERNAME", setting("turn.username"))
        buildConfigString("TURN_CREDENTIAL", setting("turn.credential"))
    }

    signingConfigs {
        // One fixed DEBUG key, committed on purpose, so every CI-built debug APK can update the previous one in place.
        // It protects nothing: never use it for a release build.
        getByName("debug") {
            storeFile = rootProject.file("keystore/debug.keystore")
            storePassword = "android"
            keyAlias = "androiddebugkey"
            keyPassword = "android"
        }
        if (releaseKeystore != null) {
            create("release") {
                storeFile = releaseKeystore
                storePassword = System.getenv("DUOCHAT_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("DUOCHAT_KEY_ALIAS")
                keyPassword = System.getenv("DUOCHAT_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        debug {
            isMinifyEnabled = false
        }
        release {
            // CI test builds (-Pduochat.ciSign=true) use the committed debug key, so they update earlier test builds in
            // place; a real release only ever uses the release key from the environment.
            signingConfig = signingConfigs.findByName("release")
                ?: if (project.findProperty("duochat.ciSign") == "true") signingConfigs.getByName("debug") else null
            // R8 drops unused library code (most of the icon set) but renames nothing: see proguard-rules.pro.
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    testOptions {
        unitTests.isIncludeAndroidResources = true
        unitTests.all {
            it.testLogging {
                exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
                events("failed")
            }
        }
    }

    sourceSets {
        // The UniFFI-generated Kotlin for the Rust core is checked in next to the crate; compile it into the app.
        getByName("main") { java.srcDir("../../../crates/privcord-ffi/bindings/kotlin") }
    }

    // Two builds from one codebase: "classic" is the original Kotlin engine; "rust" runs the shared Rust engine. The
    // rust build installs next to the classic one (its own app id and its own data), so trying it risks nothing.

    buildFeatures {
        compose = true
        buildConfig = true
    }

    // One APK per phone CPU type, each carrying only its own native libraries (the Rust core, from
    // crates/privcord-ffi built into src/main/jniLibs, and WebRTC; see docs/rust/ANDROID.md). Almost every phone
    // takes the arm64-v8a APK; armeabi-v7a is for old 32-bit phones. -Pduochat.emulator=true adds x86_64.
    splits {
        abi {
            isEnable = true
            reset()
            include("arm64-v8a", "armeabi-v7a")
            if (project.findProperty("duochat.emulator") == "true") include("x86_64")
            isUniversalApk = false
        }
    }
    packaging {
        // Compressed native libraries: the APK download is about half the size; Android unpacks them once at install.
        jniLibs { useLegacyPackaging = true }
        resources {
            excludes += setOf(
                "META-INF/AL2.0",
                "META-INF/LGPL2.1",
                "META-INF/LICENSE*",
                "META-INF/NOTICE*",
            )
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(composeBom)
    androidTestImplementation(composeBom)

    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.activity:activity-compose:1.10.0")
    implementation("androidx.fragment:fragment-ktx:1.8.5")
    implementation("androidx.biometric:biometric:1.1.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    implementation("androidx.lifecycle:lifecycle-viewmodel-ktx:2.8.7")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    debugImplementation("androidx.compose.ui:ui-tooling")
    // Render tests on the JVM (src/test): Robolectric runs the real Compose UI.
    testImplementation("androidx.compose.ui:ui-test-junit4")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
    testImplementation("org.robolectric:robolectric:4.18.1")

    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    // WebSocket client for the signaling relay and Nostr relays.
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    // The generated bindings call the Rust library through JNA.
    implementation("net.java.dev.jna:jna:5.19.1@aar")

    implementation("androidx.room:room-runtime:2.8.5")
    implementation("androidx.room:room-ktx:2.8.5")
    ksp("androidx.room:room-compiler:2.8.5")

    implementation("io.github.webrtc-sdk:android:150.7871.04")
    // Voice notes and audio files: plays the WebM/Opus that desktop browsers record (MediaPlayer often refuses it).
    implementation("androidx.media3:media3-exoplayer:1.5.1")
    // Plays received video and voice notes straight from memory, with the usual player controls.
    implementation("androidx.media3:media3-datasource:1.5.1")
    implementation("androidx.media3:media3-ui:1.5.1")
    implementation("com.google.zxing:core:3.5.4")
    // The QR scanner: CameraX for the camera, ZXing core (above) for decoding.
    implementation("androidx.camera:camera-camera2:1.4.1")
    implementation("androidx.camera:camera-lifecycle:1.7.0")
    implementation("androidx.camera:camera-view:1.4.1")

    testImplementation("junit:junit:4.13.2")
    // Real org.json for JVM unit tests (the android.jar one is a stub); reads docs/ble-vectors.json.
    testImplementation("org.json:json:20260814")
}
