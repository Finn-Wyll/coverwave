package app.coverwave.core

import android.content.Context
import android.content.SharedPreferences
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.Looper
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.SocketTimeoutException
import java.net.URL
import java.net.URLEncoder
import java.net.UnknownHostException
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** App-wide context and files, set once by `CoverwaveApp`. */
object Platform {
    lateinit var context: Context
        private set

    fun init(c: Context) {
        context = c.applicationContext
    }

    /** Kept until the app is uninstalled (like iOS Application Support). */
    val filesDir: File get() = context.filesDir

    /** Can be cleared by the system. */
    val cacheDir: File get() = context.cacheDir

    val prefs: SharedPreferences by lazy { context.getSharedPreferences("coverwave", Context.MODE_PRIVATE) }

    private val main = Handler(Looper.getMainLooper())
    fun onMain(block: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) block() else main.post(block)
    }
}

/**
 * Secrets (the Spotify tokens), encrypted with a key that never leaves the
 * Android Keystore, like the iPhone's Keychain.
 */
object Vault {
    private const val ALIAS = "coverwave.vault"
    private val prefs by lazy { Platform.context.getSharedPreferences("vault", Context.MODE_PRIVATE) }

    private fun key(): SecretKey {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (ks.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        gen.init(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .build(),
        )
        return gen.generateKey()
    }

    fun save(value: String, key: String) {
        runCatching {
            val c = Cipher.getInstance("AES/GCM/NoPadding")
            c.init(Cipher.ENCRYPT_MODE, key())
            val sealed = c.iv + c.doFinal(value.toByteArray())
            prefs.edit().putString(key, Base64.encodeToString(sealed, Base64.NO_WRAP)).apply()
        }
    }

    fun read(key: String): String? = runCatching {
        val sealed = Base64.decode(prefs.getString(key, null) ?: return null, Base64.NO_WRAP)
        val c = Cipher.getInstance("AES/GCM/NoPadding")
        c.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, sealed, 0, 12))
        String(c.doFinal(sealed, 12, sealed.size - 12))
    }.getOrNull()

    fun delete(key: String) {
        prefs.edit().remove(key).apply()
    }
}

/** Whether the internet can be reached, and over what. */
object NetworkMonitor {
    var isOnline by mutableStateOf(true)
        private set

    /** Mobile data or a hotspot (Android calls it metered). */
    var isExpensive by mutableStateOf(false)
        private set

    /** Called on the main thread after each change. */
    val onChange = mutableListOf<() -> Unit>()

    fun start(context: Context) {
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return
        fun refresh() {
            val caps = cm.activeNetwork?.let { cm.getNetworkCapabilities(it) }
            val online = caps?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
            val expensive = caps?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) == false
            Platform.onMain {
                if (online != isOnline || expensive != isExpensive) {
                    isOnline = online
                    isExpensive = expensive
                    onChange.toList().forEach { it() }
                }
            }
        }
        refresh()
        cm.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) = refresh()
            override fun onLost(network: Network) = refresh()
            override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) = refresh()
        })
    }
}

/** The request never reached the server: no network, DNS, timeout. */
val Throwable.isOffline: Boolean
    get() = this is UnknownHostException || this is SocketTimeoutException ||
        this is java.net.ConnectException || this is java.net.NoRouteToHostException ||
        (this is IOException && cause?.isOffline == true)

/** One HTTP answer. */
class HttpResponse(val code: Int, val body: ByteArray) {
    val text: String get() = String(body)
    fun json(): JSONObject = runCatching { JSONObject(text) }.getOrDefault(JSONObject())
}

/** Small blocking HTTP client (run in [Dispatchers.IO]); throws on no network. */
object Http {
    private const val USER_AGENT = "coverwave-android/0.1"

    suspend fun request(
        url: String,
        method: String = "GET",
        headers: Map<String, String> = emptyMap(),
        body: ByteArray? = null,
    ): HttpResponse = withContext(Dispatchers.IO) {
        val c = URL(url).openConnection() as HttpURLConnection
        try {
            c.requestMethod = method
            c.connectTimeout = 15_000
            c.readTimeout = 30_000
            c.setRequestProperty("User-Agent", USER_AGENT)
            headers.forEach { (k, v) -> c.setRequestProperty(k, v) }
            if (body != null) {
                c.doOutput = true
                c.outputStream.use { it.write(body) }
            } else if (method == "PUT" || method == "POST") {
                c.doOutput = true
                c.setFixedLengthStreamingMode(0)
                c.outputStream.close()
            }
            val code = c.responseCode
            val stream = if (code >= 400) c.errorStream else c.inputStream
            HttpResponse(code, stream?.use { it.readBytes() } ?: ByteArray(0))
        } finally {
            c.disconnect()
        }
    }

    /** GET a URL's bytes, or null (any failure). */
    suspend fun bytes(url: String): ByteArray? = runCatching {
        request(url).takeIf { it.code == 200 }?.body
    }.getOrNull()

    /** GET JSON from a keyless public API, or null. */
    suspend fun json(base: String, query: List<Pair<String, String>> = emptyList()): JSONObject? = runCatching {
        val r = request(withQuery(base, query))
        if (r.code == 200) JSONObject(r.text) else null
    }.getOrNull()

    fun withQuery(base: String, query: List<Pair<String, String>>): String =
        if (query.isEmpty()) base
        else base + "?" + query.joinToString("&") { (k, v) -> "${enc(k)}=${enc(v)}" }

    fun enc(s: String): String = URLEncoder.encode(s, "UTF-8").replace("+", "%20")
}

fun sha256Hex(data: ByteArray): String =
    MessageDigest.getInstance("SHA-256").digest(data).joinToString("") { "%02x".format(it) }

/** First 12 bytes of SHA-256, as file names (as on iPhone). */
fun hashed(key: String): String =
    MessageDigest.getInstance("SHA-256").digest(key.toByteArray()).take(12).joinToString("") { "%02x".format(it) }

fun JSONObject.str(key: String): String? = if (has(key) && !isNull(key)) optString(key) else null
fun JSONObject.obj(key: String): JSONObject? = optJSONObject(key)
fun JSONObject.arr(key: String): JSONArray? = optJSONArray(key)
fun JSONArray.objects(): List<JSONObject> = (0 until length()).mapNotNull { optJSONObject(it) }

/** Elapsed / remaining, as m:ss. */
fun timeText(seconds: Double): String {
    val s = maxOf(0, seconds.toInt())
    return "%d:%02d".format(s / 60, s % 60)
}

/** Writes atomically, making folders as needed. */
fun File.writeAtomically(data: ByteArray) {
    parentFile?.mkdirs()
    val tmp = File(parentFile, "$name.tmp")
    tmp.writeBytes(data)
    tmp.renameTo(this)
}
