plugins {
    id("com.android.library")
    kotlin("android")
    `maven-publish`
}

android {
    namespace = "org.cashudevkit"
    compileSdk = 34
    defaultConfig {
        minSdk = 24
    }
    publishing {
        singleVariant("release") {
            withSourcesJar()
            withJavadocJar()
        }
    }
}

dependencies {
    api(project(":cdk-jvm")) {
        exclude(group = "net.java.dev.jna", module = "jna")
    }
    runtimeOnly("net.java.dev.jna:jna:5.14.0@aar") {
        isTransitive = false
    }
}

afterEvaluate {
    publishing {
        publications {
            create<MavenPublication>("release") {
                from(components["release"])
                groupId = project.property("GROUP") as String
                artifactId = "cdk-android"
                version = project.property("VERSION_NAME") as String
            }
        }
    }
}
