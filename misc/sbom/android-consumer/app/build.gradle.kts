plugins { id("com.android.application") }

android {
    namespace = "org.cashudevkit.sbomconsumer"
    compileSdk = 34
    // The release AAR is already finalized; test exactly its bytes in the APK.
    packaging.jniLibs.keepDebugSymbols += "**/libcdk_ffi_kotlin.so"
    defaultConfig {
        applicationId = "org.cashudevkit.sbomconsumer"
        minSdk = 24
        targetSdk = 34
        versionCode = 1
        versionName = "1"
        ndk { abiFilters += "x86_64" }
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    implementation(files("libs/cdk.aar"))
    implementation("org.jetbrains.kotlin:kotlin-stdlib:1.9.24")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.8.1")
    implementation("net.java.dev.jna:jna:5.17.0@aar") { isTransitive = false }
    androidTestImplementation("androidx.test:runner:1.5.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.5")
}
