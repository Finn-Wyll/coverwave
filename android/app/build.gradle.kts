import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// The Spotify app's public client ID: SPOTIFY_CLIENT_ID in the environment
// (the Release workflow passes the repository variable), else
// `spotify.clientId` in android/local.properties, else the owner's ID in
// android/gradle.properties. Without any, the app asks for it in Settings.
val localProps = Properties().apply {
    val f = rootProject.file("local.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}
val spotifyClientId: String =
    System.getenv("SPOTIFY_CLIENT_ID")?.takeIf { it.isNotBlank() }
        ?: localProps.getProperty("spotify.clientId")?.takeIf { it.isNotBlank() }
        ?: (findProperty("coverwave.spotifyClientId") as String?).orEmpty()

android {
    namespace = "app.coverwave"
    compileSdk = 35

    defaultConfig {
        applicationId = "app.coverwave"
        minSdk = 26
        targetSdk = 35
        versionCode = (System.getenv("COVERWAVE_VERSION_CODE") ?: "1").toInt()
        versionName = System.getenv("COVERWAVE_VERSION_NAME") ?: "0.1.0"
        buildConfigField("String", "SPOTIFY_CLIENT_ID", "\"$spotifyClientId\"")
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }

    signingConfigs {
        // A fixed debug key, committed on purpose (the owner's choice), so
        // every build from any machine installs over the last one. It
        // proves nothing about who built the APK.
        create("shared") {
            storeFile = rootProject.file("coverwave-debug.keystore")
            storePassword = "android"
            keyAlias = "coverwave"
            keyPassword = "android"
        }
    }

    buildTypes {
        getByName("debug") {
            signingConfig = signingConfigs.getByName("shared")
        }
        getByName("release") {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("shared")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    buildFeatures {
        compose = true
        buildConfig = true
    }
    packaging {
        jniLibs { useLegacyPackaging = false }
    }
}

dependencies {
    // Spotify's App Remote SDK (Apache-2.0), vendored from
    // github.com/spotify/android-sdk; it needs Gson.
    implementation(files("libs/spotify-app-remote-release-0.8.0.aar"))
    implementation("com.google.code.gson:gson:2.11.0")
    // UniFFI's Kotlin bindings load the Rust core through JNA.
    implementation("net.java.dev.jna:jna:5.15.0@aar")

    val composeBom = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(composeBom)
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.browser:browser:1.8.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")

    // JVM tests of the Kotlin bindings against a host build of the core.
    testImplementation("junit:junit:4.13.2")
    testImplementation("net.java.dev.jna:jna:5.15.0")
}
