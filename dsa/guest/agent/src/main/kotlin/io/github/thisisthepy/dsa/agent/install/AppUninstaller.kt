package io.github.thisisthepy.dsa.agent.install

import io.github.thisisthepy.dsa.agent.Log
import io.github.thisisthepy.dsa.control.v1.Envelope
import io.github.thisisthepy.dsa.control.v1.UninstallResult
import io.github.thisisthepy.dsa.control.v1.UninstallStatus

/** What kind of app a package is, as far as uninstalling it goes. */
enum class AppKind {
    /** An app the user installed, which can be uninstalled. */
    USER,

    /** An app that is part of the Android image, which stays. */
    SYSTEM,
}

/** The guest's package installer, as the agent uses it to uninstall apps. */
interface PackageUninstallerPort {
    /** What kind of app [packageName] is; null when it is not installed. */
    fun kind(packageName: String): AppKind?

    /** Uninstalls [packageName]; [onResult] is called once, on any thread, when the installer has decided. */
    fun uninstall(packageName: String, onResult: (InstallerResult) -> Unit)

    companion object {
        val NONE = object : PackageUninstallerPort {
            override fun kind(packageName: String): AppKind? = null
            override fun uninstall(packageName: String, onResult: (InstallerResult) -> Unit) =
                throw IllegalStateException("this agent uninstalls no apps")
        }
    }
}

/**
 * Uninstalls the apps the host asks it to (UninstallApp), and answers each request with how it
 * went (UninstallResult of the same request ID).
 *
 * Only an app the user installed is uninstalled. An app that is part of the Android image, the
 * agent itself, and a package that is not installed are refused at once with their reason, so
 * the host can say it in the user's language. A failure of the installer is answered, never
 * thrown: a bad request never stops the agent.
 */
class AppUninstaller(
    private val ownPackage: String,
    private val port: PackageUninstallerPort,
    private val send: (ByteArray) -> Boolean,
    private val log: Log,
    /** Runs work on the agent's thread; the installer answers on any thread. */
    private val post: (() -> Unit) -> Unit,
    /** Called once an app is gone, so the host's list of apps follows at once. */
    private val onUninstalled: () -> Unit = {},
) {
    /** Takes an UninstallApp frame. False for any other frame. */
    fun handle(envelope: Envelope): Boolean {
        if (envelope.bodyCase != Envelope.BodyCase.UNINSTALL_APP) return false
        val request = envelope.uninstallApp
        val packageName = request.packageName
        val kind = try {
            if (packageName.isEmpty() || packageName == ownPackage) null else port.kind(packageName)
        } catch (e: Exception) {
            log.warn("could not ask the package manager about $packageName", e)
            answer(request.requestId, packageName, UninstallStatus.UNINSTALL_STATUS_OTHER, "the package manager could not be asked: ${e.message}")
            return true
        }
        when {
            packageName == ownPackage && packageName.isNotEmpty() ->
                answer(request.requestId, packageName, UninstallStatus.UNINSTALL_STATUS_SYSTEM_APP, "the agent is part of the Android image")
            kind == null ->
                answer(request.requestId, packageName, UninstallStatus.UNINSTALL_STATUS_NOT_INSTALLED, "no app of this package is installed")
            kind == AppKind.SYSTEM ->
                answer(request.requestId, packageName, UninstallStatus.UNINSTALL_STATUS_SYSTEM_APP, "the app is part of the Android image")
            else -> start(request.requestId, packageName)
        }
        return true
    }

    private fun start(requestId: Int, packageName: String) {
        log.info("uninstalling $packageName")
        try {
            port.uninstall(packageName) { result -> post { finished(requestId, packageName, result) } }
        } catch (e: Exception) {
            log.warn("the package installer refused to uninstall $packageName", e)
            answer(requestId, packageName, UninstallStatus.UNINSTALL_STATUS_OTHER, "the package installer refused: ${e.message}")
        }
    }

    private fun finished(requestId: Int, packageName: String, result: InstallerResult) {
        val status = reason(result.status)
        val message = result.message.orEmpty()
        if (status == UninstallStatus.UNINSTALL_STATUS_SUCCESS) {
            log.info("uninstalled $packageName")
            onUninstalled()
        } else {
            log.warn("did not uninstall $packageName: $message")
        }
        answer(requestId, packageName, status, message)
    }

    private fun answer(requestId: Int, packageName: String, status: UninstallStatus, message: String) {
        val result = UninstallResult.newBuilder()
            .setRequestId(requestId)
            .setPackageName(packageName)
            .setStatus(status)
            .setMessage(message)
        send(Envelope.newBuilder().setUninstallResult(result).build().toByteArray())
    }

    companion object {
        /** The stable reason the host is told for what the installer said. */
        fun reason(status: Int): UninstallStatus = when (status) {
            PackageInstallerStatus.SUCCESS -> UninstallStatus.UNINSTALL_STATUS_SUCCESS
            PackageInstallerStatus.FAILURE_BLOCKED -> UninstallStatus.UNINSTALL_STATUS_BLOCKED
            else -> UninstallStatus.UNINSTALL_STATUS_OTHER
        }
    }
}
