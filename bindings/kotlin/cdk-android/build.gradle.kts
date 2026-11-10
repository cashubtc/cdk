plugins {
    id("com.android.library")
    kotlin("android")
    `maven-publish`
}

val cdkCandidate = providers.gradleProperty("cdkReleaseRepository").isPresent
val candidateJavadocJar = if (cdkCandidate) {
    apply(plugin = "org.jetbrains.dokka")
    val docs = tasks.named<org.jetbrains.dokka.gradle.DokkaTask>("dokkaJavadoc") {
        offlineMode.set(true)
        dokkaSourceSets.configureEach {
            noJdkLink.set(true)
            noStdlibLink.set(true)
            noAndroidSdkLink.set(true)
        }
    }
    tasks.register<Jar>("candidateJavadocJar") {
        archiveClassifier.set("javadoc")
        from(docs.flatMap { it.outputDirectory })
    }
} else {
    null
}

android {
    namespace = "org.cashudevkit"
    compileSdk = 34
    defaultConfig {
        minSdk = 24
    }
    // Align Java and Kotlin JVM targets (matches cdk-jvm). Without this the
    // Kotlin compile defaults to the host JDK and clashes with AGP's Java 8.
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    // The uniffi-generated Kotlin lives in the sibling cdk-jvm module (shared
    // with the desktop tests). Compile it straight into the AAR so the only
    // artifact published to Maven Central is self-contained and needs no
    // cdk-jvm coordinate to resolve.
    sourceSets {
        getByName("main") {
            java.srcDir("../cdk-jvm/src/main/kotlin")
        }
    }
    publishing {
        singleVariant("release") {
            withSourcesJar()
            if (!cdkCandidate) {
                withJavadocJar()
            }
        }
    }
}

dependencies {
    // The generated bindings expose suspend functions, so downstream consumers
    // need coroutines on their compile classpath.
    api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.8.1")
    // JNA loads the native library and ships the Android dispatch libs in its
    // AAR. Needed at compile time now that the bindings are compiled here.
    implementation("net.java.dev.jna:jna:5.17.0@aar") {
        isTransitive = false
    }
}

afterEvaluate {
    publishing {
        publications {
            create<MavenPublication>("release") {
                from(components["release"])
                candidateJavadocJar?.let { artifact(it) }
                groupId = project.property("GROUP") as String
                artifactId = "cdk-android"
                version = project.property("VERSION_NAME") as String
            }
        }
    }
}
