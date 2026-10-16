pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
    plugins {
        kotlin("android") version "1.9.24"
        id("com.android.library") version "8.5.1"
    }
}

dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}

plugins {
    id("org.gradle.toolchains.foojay-resolver-convention") version "0.8.0"
}

rootProject.name = "cdk"
include("cdk-jvm")

val cdkJvmOnly = providers.gradleProperty("cdkJvmOnly")
    .map(String::toBoolean)
    .getOrElse(false)

if (!cdkJvmOnly) {
    include("cdk-android")
}
