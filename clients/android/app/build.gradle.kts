import java.util.Properties
import javax.inject.Inject
import org.gradle.process.ExecOperations

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

// ---- The Rust core ----
//
// `acetate-ffi` is built by cargo-ndk into one libacetate_ffi.so per ABI, and uniffi-bindgen
// writes the Kotlin that calls it. Always the release profile: a debug Rust build is far
// slower, and a phone measured with it tells nothing. Cargo decides what is out of date, so
// both tasks run every time (a no-op when nothing changed).
//
// ABIs: arm64-v8a (phones); add x86_64 for the emulator with -Pacetate.abis=arm64-v8a,x86_64.

val clients: Directory = rootProject.layout.projectDirectory.dir("..")

abstract class CargoNdk @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    @get:Input abstract val abis: ListProperty<String>
    @get:Input abstract val minSdk: Property<Int>
    @get:Internal abstract val workspace: DirectoryProperty
    @get:Internal abstract val ndk: DirectoryProperty
    @get:Internal abstract val sdk: DirectoryProperty
    @get:OutputDirectory abstract val outDir: DirectoryProperty

    @TaskAction
    fun build() {
        exec.exec {
            workingDir = workspace.get().asFile
            environment("ANDROID_HOME", sdk.get().asFile.path)
            environment("ANDROID_NDK_HOME", ndk.get().asFile.path)
            environment("PATH", "${System.getProperty("user.home")}/.cargo/bin:${System.getenv("PATH")}")
            val args = mutableListOf("cargo", "ndk")
            abis.get().forEach { args += listOf("-t", it) }
            args += listOf("-P", minSdk.get().toString(), "-o", outDir.get().asFile.path,
                "build", "-p", "acetate-ffi", "--release")
            commandLine(args)
        }
    }
}

abstract class UniffiBindgen @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    @get:InputDirectory abstract val libs: DirectoryProperty
    @get:Internal abstract val workspace: DirectoryProperty
    @get:OutputDirectory abstract val outDir: DirectoryProperty

    @TaskAction
    fun generate() {
        val lib = libs.get().asFile.walk().first { it.name == "libacetate_ffi.so" }
        outDir.get().asFile.deleteRecursively()
        exec.exec {
            workingDir = workspace.get().asFile
            environment("PATH", "${System.getProperty("user.home")}/.cargo/bin:${System.getenv("PATH")}")
            commandLine("cargo", "run", "-q", "-p", "acetate-ffi", "--bin", "uniffi-bindgen", "--",
                "generate", "--library", lib.path, "--language", "kotlin", "--no-format",
                "--out-dir", outDir.get().asFile.path)
        }
    }
}

// ---- The fonts ----
//
// Inter is kept once, in clients/assets/fonts (the desktop includes it from there); the app
// gets it as font resources (names lowercase, words joined with _).
abstract class InterFonts @Inject constructor(private val files: FileSystemOperations) : DefaultTask() {
    @get:InputDirectory abstract val fonts: DirectoryProperty
    @get:OutputDirectory abstract val outDir: DirectoryProperty

    @TaskAction
    fun copy() {
        files.sync {
            from(fonts) { include("Inter-*.ttf") }
            into(outDir.dir("font"))
            rename { it.removeSuffix(".ttf").replace("Inter-", "inter_").lowercase() + ".ttf" }
        }
    }
}

android {
    namespace = "io.github.allinen.acetate"
    compileSdk { version = release(37) }
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "io.github.allinen.acetate"
        minSdk = 29
        targetSdk = 36
        // -Pacetate.versionCode=2 -Pacetate.versionName=0.2.0 for a release.
        versionCode = (findProperty("acetate.versionCode") as String?)?.toInt() ?: 1
        versionName = (findProperty("acetate.versionName") as String?) ?: "0.2.1"
    }

    // The key a release is signed with: `keystore.properties` next to settings.gradle.kts
    // (git-ignored: storeFile, storePassword, keyAlias, keyPassword), or the environment
    // (ACETATE_KEYSTORE, ACETATE_KEYSTORE_PASSWORD, ACETATE_KEY_ALIAS, ACETATE_KEY_PASSWORD).
    // Without one, a release is signed with the debug key: it installs, but it isn't one to
    // publish (see docs/android.md).
    val keys = Properties().apply {
        val file = rootProject.file("keystore.properties")
        if (file.exists()) file.inputStream().use { load(it) }
    }
    fun key(name: String, env: String): String? = keys.getProperty(name) ?: System.getenv(env)
    val releaseKey = listOf(
        key("storeFile", "ACETATE_KEYSTORE"), key("storePassword", "ACETATE_KEYSTORE_PASSWORD"),
        key("keyAlias", "ACETATE_KEY_ALIAS"), key("keyPassword", "ACETATE_KEY_PASSWORD"),
    )
    val hasReleaseKey = releaseKey.all { it != null }
    signingConfigs {
        if (hasReleaseKey) create("release") {
            storeFile = rootProject.file(releaseKey[0]!!)
            storePassword = releaseKey[1]
            keyAlias = releaseKey[2]
            keyPassword = releaseKey[3]
        }
    }

    buildTypes {
        debug {
            // Installs next to the release build (own login, own downloads), for testing against a dev server.
            applicationIdSuffix = ".debug"
        }
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = if (hasReleaseKey) signingConfigs.getByName("release") else signingConfigs.getByName("debug")
        }
        // The release as built for publishing, but installable next to the others and signed with
        // the debug key: for trying what shrinking does before a release.
        create("stage") {
            initWith(getByName("release"))
            applicationIdSuffix = ".stage"
            signingConfig = signingConfigs.getByName("debug")
            matchingFallbacks += "release"
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

val targetAbis = (findProperty("acetate.abis") as String? ?: "arm64-v8a").split(",")

val cargoNdk = tasks.register<CargoNdk>("cargoNdk") {
    abis.set(targetAbis)
    minSdk.set(android.defaultConfig.minSdk!!)
    workspace.set(clients)
    ndk.set(androidComponents.sdkComponents.ndkDirectory)
    sdk.set(androidComponents.sdkComponents.sdkDirectory)
    outDir.set(layout.buildDirectory.dir("rust/jniLibs"))
    outputs.upToDateWhen { false }
}

val bindgen = tasks.register<UniffiBindgen>("uniffiBindgen") {
    libs.set(cargoNdk.flatMap { it.outDir })
    workspace.set(clients)
    outDir.set(layout.buildDirectory.dir("generated/uniffi"))
}

val interFonts = tasks.register<InterFonts>("interFonts") {
    fonts.set(clients.dir("assets/fonts"))
    outDir.set(layout.buildDirectory.dir("generated/fonts"))
}

androidComponents {
    onVariants { variant ->
        variant.sources.res!!.addGeneratedSourceDirectory(interFonts, InterFonts::outDir)
        variant.sources.jniLibs!!.addGeneratedSourceDirectory(cargoNdk, CargoNdk::outDir)
        variant.sources.kotlin!!.addGeneratedSourceDirectory(bindgen, UniffiBindgen::outDir)
    }
}

dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
    implementation(libs.compose.material3)
    implementation(libs.compose.icons.core)
    implementation(libs.media3.exoplayer)
    implementation(libs.media3.session)
    implementation(libs.media3.okhttp)
    implementation(libs.coil.compose)
    implementation(libs.coil.network.okhttp)
    // UniFFI's Kotlin calls the library through JNA (the aar carries its native part).
    implementation("${libs.jna.get()}@aar")
}
