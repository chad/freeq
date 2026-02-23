package com.freeq.ui.components

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil.compose.AsyncImage
import com.freeq.model.AppState
import com.freeq.ui.theme.FreeqColors
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID

// ── Photo picker button for ComposeBar ──

@Composable
fun PhotoPickerButton(
    appState: AppState,
    onPhotoPicked: (Uri) -> Unit
) {
    val launcher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.PickVisualMedia()
    ) { uri -> uri?.let(onPhotoPicked) }

    val isAuthenticated = appState.authenticatedDID.value != null

    if (isAuthenticated) {
        IconButton(
            onClick = {
                launcher.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
            },
            modifier = Modifier.size(40.dp)
        ) {
            Icon(
                Icons.Default.Image,
                contentDescription = "Send photo",
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.size(22.dp)
            )
        }
    }
}

// ── Image preview sheet ──

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ImagePreviewSheet(
    uri: Uri,
    appState: AppState,
    onDismiss: () -> Unit,
    onSent: () -> Unit
) {
    val context = LocalContext.current
    val haptic = LocalHapticFeedback.current
    val scope = rememberCoroutineScope()

    var caption by remember { mutableStateOf("") }
    var crossPost by remember { mutableStateOf(false) }
    var uploading by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    val channel = appState.activeChannel.value
    val did = appState.authenticatedDID.value

    ModalBottomSheet(
        onDismissRequest = { if (!uploading) onDismiss() },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 20.dp)
                .padding(bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp)
        ) {
            // Title
            Text(
                "Send Photo",
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.align(Alignment.CenterHorizontally)
            )

            // Image preview
            AsyncImage(
                model = uri,
                contentDescription = "Selected photo",
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = 300.dp)
                    .clip(RoundedCornerShape(12.dp)),
                contentScale = ContentScale.Fit
            )

            // Caption field
            OutlinedTextField(
                value = caption,
                onValueChange = { caption = it },
                modifier = Modifier.fillMaxWidth(),
                placeholder = { Text("Add a caption...", fontSize = 15.sp) },
                maxLines = 4,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                shape = RoundedCornerShape(12.dp),
                colors = OutlinedTextFieldDefaults.colors(
                    focusedBorderColor = MaterialTheme.colorScheme.outline.copy(alpha = 0.5f),
                    unfocusedBorderColor = MaterialTheme.colorScheme.outline.copy(alpha = 0.3f),
                ),
                textStyle = LocalTextStyle.current.copy(fontSize = 15.sp)
            )

            // Cross-post toggle
            if (did != null) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween
                ) {
                    Text(
                        "Also post to Bluesky",
                        fontSize = 14.sp,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Switch(
                        checked = crossPost,
                        onCheckedChange = { crossPost = it }
                    )
                }
            }

            // Error banner
            error?.let { msg ->
                Surface(
                    color = MaterialTheme.colorScheme.errorContainer,
                    shape = RoundedCornerShape(8.dp),
                    modifier = Modifier.fillMaxWidth()
                ) {
                    Row(
                        modifier = Modifier.padding(12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp)
                    ) {
                        Icon(
                            Icons.Default.Warning,
                            contentDescription = null,
                            modifier = Modifier.size(16.dp),
                            tint = MaterialTheme.colorScheme.error
                        )
                        Text(
                            msg,
                            fontSize = 13.sp,
                            color = MaterialTheme.colorScheme.onErrorContainer,
                            modifier = Modifier.weight(1f)
                        )
                    }
                }
            }

            // Upload / send button
            Button(
                onClick = {
                    if (uploading || did == null || channel == null) return@Button
                    error = null
                    uploading = true

                    scope.launch {
                        val result = withContext(Dispatchers.IO) {
                            uploadPhoto(context, uri, did, channel, caption.trim(), crossPost)
                        }

                        result.onSuccess { url ->
                            val messageText = if (caption.isBlank()) url
                                else "$url $caption"
                            appState.sendMessage(channel, messageText.trim())
                            haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                            onSent()
                        }.onFailure { e ->
                            error = e.message ?: "Upload failed"
                            uploading = false
                        }
                    }
                },
                enabled = !uploading && did != null && channel != null,
                modifier = Modifier
                    .fillMaxWidth()
                    .height(48.dp),
                shape = RoundedCornerShape(24.dp),
                colors = ButtonDefaults.buttonColors(
                    containerColor = FreeqColors.accent
                )
            ) {
                if (uploading) {
                    CircularProgressIndicator(
                        modifier = Modifier.size(20.dp),
                        strokeWidth = 2.dp,
                        color = MaterialTheme.colorScheme.onPrimary
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Text("Uploading...")
                } else {
                    Icon(
                        Icons.Default.ArrowUpward,
                        contentDescription = null,
                        modifier = Modifier.size(18.dp)
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Text("Send")
                }
            }
        }
    }
}

// ── Upload logic ──

private fun uploadPhoto(
    context: Context,
    uri: Uri,
    did: String,
    channel: String,
    caption: String,
    crossPost: Boolean
): Result<String> {
    return try {
        // Read image bytes
        val imageBytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() }
            ?: return Result.failure(Exception("Could not read image"))

        // Detect content type
        val contentType = context.contentResolver.getType(uri) ?: "image/jpeg"

        // Compress JPEG if it's large (> 5MB)
        val finalBytes = if (imageBytes.size > 5 * 1024 * 1024 && contentType.startsWith("image/")) {
            compressImage(imageBytes) ?: imageBytes
        } else {
            imageBytes
        }

        if (finalBytes.size > 10 * 1024 * 1024) {
            return Result.failure(Exception("Image is too large (max 10MB)"))
        }

        // Build multipart request
        val boundary = UUID.randomUUID().toString()
        val url = URL("https://irc.freeq.at/api/v1/upload")
        val conn = (url.openConnection() as HttpURLConnection).apply {
            requestMethod = "POST"
            doOutput = true
            connectTimeout = 30_000
            readTimeout = 30_000
            setRequestProperty("Content-Type", "multipart/form-data; boundary=$boundary")
        }

        conn.outputStream.use { out ->
            fun writeField(name: String, value: String) {
                out.write("--$boundary\r\n".toByteArray())
                out.write("Content-Disposition: form-data; name=\"$name\"\r\n\r\n".toByteArray())
                out.write("$value\r\n".toByteArray())
            }

            writeField("did", did)
            writeField("channel", channel)
            if (crossPost) writeField("cross_post", "true")
            if (caption.isNotEmpty()) writeField("alt", caption)

            // File field
            out.write("--$boundary\r\n".toByteArray())
            out.write("Content-Disposition: form-data; name=\"file\"; filename=\"photo.jpg\"\r\n".toByteArray())
            out.write("Content-Type: $contentType\r\n\r\n".toByteArray())
            out.write(finalBytes)
            out.write("\r\n".toByteArray())

            // End boundary
            out.write("--$boundary--\r\n".toByteArray())
            out.flush()
        }

        val responseCode = conn.responseCode
        if (responseCode == 200) {
            val body = conn.inputStream.bufferedReader().readText()
            val json = JSONObject(body)
            Result.success(json.getString("url"))
        } else {
            val errBody = try {
                conn.errorStream?.bufferedReader()?.readText()?.take(80) ?: ""
            } catch (_: Exception) { "" }
            val msg = when (responseCode) {
                401 -> "Not authorized — try logging in again"
                413 -> "Image is too large"
                else -> "Upload failed ($responseCode) $errBody".trim()
            }
            Result.failure(Exception(msg))
        }
    } catch (e: java.net.SocketTimeoutException) {
        Result.failure(Exception("Upload timed out — tap Send to retry"))
    } catch (e: Exception) {
        Result.failure(Exception("Upload failed: ${e.message}"))
    }
}

private fun compressImage(bytes: ByteArray): ByteArray? {
    return try {
        val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size) ?: return null
        val out = ByteArrayOutputStream()
        bitmap.compress(Bitmap.CompressFormat.JPEG, 85, out)
        bitmap.recycle()
        out.toByteArray()
    } catch (_: Exception) {
        null
    }
}
