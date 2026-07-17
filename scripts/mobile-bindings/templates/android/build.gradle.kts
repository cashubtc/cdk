plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "org.cashudevkit"
    compileSdk = 37

    defaultConfig {
        minSdk = 26
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlin {
        jvmToolchain(17)
    }
}

dependencies {
    api("net.java.dev.jna:jna:5.17.0@aar") {
        isTransitive = false
    }
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")
}
