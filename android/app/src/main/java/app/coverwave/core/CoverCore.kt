package app.coverwave.core

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.cw_ffi.AnalysedCover
import uniffi.cw_ffi.CoverEngine
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.MessageDigest

/**
 * The Rust core's cover engine, plus the CLIP model it needs.
 *
 * The model (MobileCLIP-S0's vision tower, 45 MB) is downloaded from Hugging
 * Face on first use, checked against a pinned SHA-256 and kept in the app's
 * files. Its weights are licensed for research and personal use only, which
 * is why it is downloaded rather than bundled. Without it, covers still
 * move, using neutral scenes.
 */
class CoverCore {
    sealed class ModelState {
        data object Missing : ModelState()
        data class Downloading(val fraction: Double) : ModelState()
        data object Loading : ModelState()
        data object Ready : ModelState()
        data class Failed(val message: String) : ModelState()
    }

    companion object {
        // Same pinned file as the terminal and iPhone apps.
        private const val MODEL_URL =
            "https://huggingface.co/Xenova/mobileclip_s0/resolve/757d59c9c6870a76a4b0306f05f5061bca15c39f/onnx/vision_model.onnx"
        private const val MODEL_SHA256 = "17d3c037b1d488c10c50e09f6009ea5a198caef4e0e8f4ea5617b7cb2d067ac0"
        const val MODEL_SIZE_TEXT = "45 MB"
    }

    var modelState by mutableStateOf<ModelState>(ModelState.Missing)
        private set

    val engine = CoverEngine()

    private val modelFile: File get() = File(Platform.filesDir, "models/mobileclip_s0_vision.onnx")

    /** Loads the model if it was downloaded before. */
    suspend fun loadIfPresent() {
        if (modelState == ModelState.Missing && modelFile.exists()) load()
    }

    suspend fun download() {
        if (modelState !is ModelState.Missing && modelState !is ModelState.Failed) return
        modelState = ModelState.Downloading(0.0)
        try {
            withContext(Dispatchers.IO) {
                val tmp = File(Platform.cacheDir, "model.download")
                val c = URL(MODEL_URL).openConnection() as HttpURLConnection
                c.connectTimeout = 15_000
                c.readTimeout = 60_000
                try {
                    if (c.responseCode != 200) throw IllegalStateException("the download failed")
                    val total = c.contentLengthLong.takeIf { it > 0 }
                    val digest = MessageDigest.getInstance("SHA-256")
                    var done = 0L
                    var lastShown = -1
                    c.inputStream.use { input ->
                        tmp.outputStream().use { out ->
                            val buf = ByteArray(1 shl 16)
                            while (true) {
                                val n = input.read(buf)
                                if (n < 0) break
                                out.write(buf, 0, n)
                                digest.update(buf, 0, n)
                                done += n
                                if (total != null) {
                                    val pct = (done * 100 / total).toInt()
                                    if (pct != lastShown) {
                                        lastShown = pct
                                        Platform.onMain { modelState = ModelState.Downloading(pct / 100.0) }
                                    }
                                }
                            }
                        }
                    }
                    val hex = digest.digest().joinToString("") { "%02x".format(it) }
                    if (hex != MODEL_SHA256) throw IllegalStateException("the downloaded model is not the expected file")
                    modelFile.parentFile?.mkdirs()
                    modelFile.delete()
                    if (!tmp.renameTo(modelFile)) tmp.copyTo(modelFile, overwrite = true)
                } finally {
                    c.disconnect()
                    tmp.delete()
                }
            }
            load()
        } catch (e: Exception) {
            modelState = ModelState.Failed(e.message ?: "the download failed")
        }
    }

    private suspend fun load() {
        modelState = ModelState.Loading
        try {
            withContext(Dispatchers.Default) { engine.loadModel(modelFile.readBytes()) }
            modelState = ModelState.Ready
        } catch (e: Exception) {
            modelFile.delete()
            modelState = ModelState.Failed(e.message ?: "the model could not be loaded")
        }
    }

    /**
     * Analyses a cover off the main thread. The CLIP embedding is cached by
     * cover hash, so a cover is only run through the model once.
     */
    suspend fun analyse(image: ByteArray, genre: String?): AnalysedCover = withContext(Dispatchers.Default) {
        val hash = sha256Hex(image)
        val cached = EmbeddingCache.load(hash)
        val cover = engine.analyse(image, genre, cached)
        if (cached == null) cover.embedding()?.let { EmbeddingCache.save(it, hash) }
        cover
    }
}

/** CLIP embeddings by cover hash (512 floats each), in the cache folder. */
object EmbeddingCache {
    private val folder get() = File(Platform.cacheDir, "embeddings")

    fun load(hash: String): List<Float>? {
        val f = File(folder, "$hash.f32")
        if (!f.exists()) return null
        val bytes = f.readBytes()
        if (bytes.isEmpty() || bytes.size % 4 != 0) return null
        val fb = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN).asFloatBuffer()
        return FloatArray(fb.remaining()).also { fb.get(it) }.asList()
    }

    fun save(embedding: List<Float>, hash: String) {
        val bb = ByteBuffer.allocate(embedding.size * 4).order(ByteOrder.LITTLE_ENDIAN)
        embedding.forEach { bb.putFloat(it) }
        File(folder, "$hash.f32").writeAtomically(bb.array())
    }
}
