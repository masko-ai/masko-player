group = "ai.masko"
version = "0.1.14"

buildscript {
    val kotlinVersion = "2.4.0"
    repositories {
        google()
        mavenCentral()
    }

    dependencies {
        classpath("com.android.tools.build:gradle:9.1.0")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:$kotlinVersion")
    }
}

allprojects {
    repositories {
        google()
        mavenCentral()
    }
}

plugins {
    id("com.android.library")
}

// The Kotlin player (android/player in the masko-player repository), its
// bindings and its engine libraries build into this plugin from player/,
// which scripts/build-flutter.sh copies here and the package publishes. No
// Maven repository is involved.
val player = file("player")
for (part in listOf("java/ai/masko/player", "generated/ai/masko/player/engine", "jniLibs/arm64-v8a")) {
    if (!player.resolve(part).isDirectory) {
        throw GradleException(
            "The Kotlin player is missing $part: run scripts/build-flutter.sh in the masko-player repository.",
        )
    }
}

android {
    namespace = "ai.masko.player.flutter"

    compileSdk = 36

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets {
        getByName("main") {
            java.srcDirs("src/main/kotlin", "player/java", "player/generated")
            jniLibs.srcDirs("player/jniLibs")
        }
    }

    defaultConfig {
        minSdk = 24
        consumerProguardFiles("player/consumer-rules.pro")
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

// The Kotlin player's own dependencies, as in android/player/build.gradle.kts.
dependencies {
    implementation("androidx.media3:media3-exoplayer:1.6.1")
    implementation("androidx.media3:media3-datasource:1.6.1")
    implementation("androidx.media3:media3-database:1.6.1")
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    implementation("androidx.annotation:annotation:1.9.1")
}
