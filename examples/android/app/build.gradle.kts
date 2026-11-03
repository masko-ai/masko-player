plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "ai.masko.player.demo"
    compileSdk = 35

    defaultConfig {
        applicationId = "ai.masko.player.demo"
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        // The engine is built for these; JNA alone also ships others.
        ndk { abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64") }
    }

    sourceSets {
        // The library's stacked test clips, which the bundled demo delivery plays.
        getByName("main").assets.srcDir("../../../android/player/src/androidTest/assets")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation("ai.masko:player:0.1.14")
}
