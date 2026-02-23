package com.freeq.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Favorite
import androidx.compose.material.icons.filled.Link
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Repeat
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil.compose.AsyncImage
import coil.compose.SubcomposeAsyncImage
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.net.URI
import java.net.URL
import java.net.URLEncoder

private val IMAGE_PATTERN = Regex(
    """https?://\S+\.(?:png|jpg|jpeg|gif|webp)(?:\?\S*)?""",
    RegexOption.IGNORE_CASE
)
private val CDN_PATTERN = Regex(
    """https?://cdn\.bsky\.app/img/[^\s<]+""",
    RegexOption.IGNORE_CASE
)
private val YOUTUBE_PATTERN = Regex(
    """(?:youtube\.com/watch\?v=|youtu\.be/)([a-zA-Z0-9_-]{11})"""
)
private val BSKY_POST_PATTERN = Regex(
    """https?://bsky\.app/profile/([^/]+)/post/([a-zA-Z0-9]+)"""
)
private val URL_PATTERN = Regex(
    """https?://\S+"""
)

@Composable
fun MessageContent(
    text: String,
    isAction: Boolean,
    fromNick: String,
    onImageClick: ((String) -> Unit)? = null
) {
    val uriHandler = LocalUriHandler.current

    // Priority: image > Bluesky post > YouTube > generic link
    val imageUrl = IMAGE_PATTERN.find(text)?.value ?: CDN_PATTERN.find(text)?.value
    val bskyMatch = if (imageUrl == null) BSKY_POST_PATTERN.find(text) else null
    val ytMatch = if (imageUrl == null && bskyMatch == null) YOUTUBE_PATTERN.find(text) else null
    val linkUrl = if (imageUrl == null && bskyMatch == null && ytMatch == null) URL_PATTERN.find(text)?.value else null

    val embedUrl = imageUrl
        ?: bskyMatch?.let { URL_PATTERN.find(text)?.value }
        ?: ytMatch?.let { URL_PATTERN.find(text)?.value }
        ?: linkUrl
    val remainingText = embedUrl?.let { text.replace(it, "").trim() } ?: text

    // Text portion
    val showText = if (embedUrl != null) remainingText else text
    if (showText.isNotEmpty()) {
        SelectionContainer {
            if (isAction) {
                Text(
                    text = "$fromNick $showText",
                    fontSize = 15.sp,
                    fontStyle = FontStyle.Italic,
                    color = MaterialTheme.colorScheme.onBackground
                )
            } else {
                Text(
                    text = showText,
                    fontSize = 15.sp,
                    color = MaterialTheme.colorScheme.onBackground
                )
            }
        }
    } else if (isAction && embedUrl != null) {
        // Action with only a URL — still show nick
        Text(
            text = fromNick,
            fontSize = 15.sp,
            fontStyle = FontStyle.Italic,
            color = MaterialTheme.colorScheme.onBackground
        )
    }

    // Embed
    when {
        imageUrl != null -> {
            InlineImage(url = imageUrl, onClick = { onImageClick?.invoke(imageUrl) })
        }
        bskyMatch != null -> {
            val handle = bskyMatch.groupValues[1]
            val rkey = bskyMatch.groupValues[2]
            val postUrl = bskyMatch.value
            BlueskyEmbed(
                handle = handle,
                rkey = rkey,
                onClick = { uriHandler.openUri(postUrl) },
                onImageClick = onImageClick
            )
        }
        ytMatch != null -> {
            val videoId = ytMatch.groupValues[1]
            YouTubeThumbnail(
                videoId = videoId,
                onClick = { uriHandler.openUri("https://youtube.com/watch?v=$videoId") }
            )
        }
        linkUrl != null -> {
            LinkPreview(url = linkUrl, onClick = { uriHandler.openUri(linkUrl) })
        }
    }
}

@Composable
private fun InlineImage(url: String, onClick: () -> Unit) {
    SubcomposeAsyncImage(
        model = url,
        contentDescription = null,
        modifier = Modifier
            .padding(top = 4.dp)
            .widthIn(max = 280.dp)
            .heightIn(max = 280.dp)
            .clip(RoundedCornerShape(8.dp))
            .clickable(onClick = onClick),
        contentScale = ContentScale.Fit,
        loading = {
            Box(
                modifier = Modifier
                    .size(120.dp, 80.dp)
                    .clip(RoundedCornerShape(8.dp))
                    .background(MaterialTheme.colorScheme.surfaceVariant),
                contentAlignment = Alignment.Center
            ) {
                CircularProgressIndicator(
                    modifier = Modifier.size(20.dp),
                    strokeWidth = 2.dp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.5f)
                )
            }
        },
        error = {
            Text(
                text = url,
                fontSize = 15.sp,
                color = MaterialTheme.colorScheme.primary
            )
        }
    )
}

@Composable
private fun YouTubeThumbnail(videoId: String, onClick: () -> Unit) {
    val thumbnailUrl = "https://img.youtube.com/vi/$videoId/mqdefault.jpg"

    Box(
        modifier = Modifier
            .padding(top = 4.dp)
            .widthIn(max = 280.dp)
            .clip(RoundedCornerShape(10.dp))
            .border(1.dp, MaterialTheme.colorScheme.outline.copy(alpha = 0.3f), RoundedCornerShape(10.dp))
            .clickable(onClick = onClick)
    ) {
        AsyncImage(
            model = thumbnailUrl,
            contentDescription = "YouTube video",
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(16f / 9f),
            contentScale = ContentScale.Crop
        )

        // Play button overlay
        Box(
            modifier = Modifier
                .align(Alignment.Center)
                .size(48.dp)
                .background(Color(0xCCFF0000), CircleShape),
            contentAlignment = Alignment.Center
        ) {
            Icon(
                Icons.Default.PlayArrow,
                contentDescription = "Play",
                tint = Color.White,
                modifier = Modifier.size(28.dp)
            )
        }
    }
}

@Composable
private fun LinkPreview(url: String, onClick: () -> Unit) {
    val domain = try {
        URI(url).host?.removePrefix("www.") ?: url
    } catch (_: Exception) { url }

    val path = try {
        val p = URI(url).path ?: ""
        if (p.length > 50) p.take(50) + "..." else p
    } catch (_: Exception) { "" }

    Row(
        modifier = Modifier
            .padding(top = 4.dp)
            .widthIn(max = 300.dp)
            .clip(RoundedCornerShape(10.dp))
            .border(1.dp, MaterialTheme.colorScheme.outline.copy(alpha = 0.3f), RoundedCornerShape(10.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .clickable(onClick = onClick)
            .padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp)
    ) {
        Box(
            modifier = Modifier
                .size(28.dp)
                .background(
                    MaterialTheme.colorScheme.primary.copy(alpha = 0.1f),
                    RoundedCornerShape(6.dp)
                ),
            contentAlignment = Alignment.Center
        ) {
            Icon(
                Icons.Default.Link,
                contentDescription = null,
                modifier = Modifier.size(16.dp),
                tint = MaterialTheme.colorScheme.primary
            )
        }

        Column(modifier = Modifier.weight(1f)) {
            Text(
                text = domain,
                fontSize = 13.sp,
                fontWeight = FontWeight.Medium,
                color = MaterialTheme.colorScheme.onBackground,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis
            )
            if (path.isNotEmpty() && path != "/") {
                Text(
                    text = path,
                    fontSize = 11.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis
                )
            }
        }
    }
}

// ── Bluesky post embed ──

private data class BskyPost(
    val authorName: String,
    val authorHandle: String,
    val authorAvatar: String?,
    val text: String,
    val imageUrl: String?,
    val likeCount: Int,
    val repostCount: Int
)

@Composable
private fun BlueskyEmbed(
    handle: String,
    rkey: String,
    onClick: () -> Unit,
    onImageClick: ((String) -> Unit)? = null
) {
    var post by remember { mutableStateOf<BskyPost?>(null) }
    var failed by remember { mutableStateOf(false) }

    LaunchedEffect(handle, rkey) {
        post = withContext(Dispatchers.IO) { fetchBskyPost(handle, rkey) }
        if (post == null) failed = true
    }

    if (failed) {
        // Fall back to link preview
        LinkPreview(url = "https://bsky.app/profile/$handle/post/$rkey", onClick = onClick)
        return
    }

    val p = post ?: run {
        // Loading
        Row(
            modifier = Modifier.padding(top = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            CircularProgressIndicator(
                modifier = Modifier.size(16.dp),
                strokeWidth = 2.dp,
                color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.5f)
            )
            Text(
                "Loading Bluesky post...",
                fontSize = 12.sp,
                color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f)
            )
        }
        return
    }

    Column(
        modifier = Modifier
            .padding(top = 4.dp)
            .widthIn(max = 300.dp)
            .clip(RoundedCornerShape(12.dp))
            .border(1.dp, MaterialTheme.colorScheme.outline.copy(alpha = 0.3f), RoundedCornerShape(12.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .clickable(onClick = onClick)
            .padding(12.dp)
    ) {
        // Author row
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            UserAvatar(nick = p.authorHandle, size = 20.dp)
            Text(
                text = p.authorName.ifEmpty { p.authorHandle },
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
                color = MaterialTheme.colorScheme.onBackground,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f)
            )
            // Bluesky butterfly icon (blue)
            Text(
                text = "\uD83E\uDD8B",
                fontSize = 14.sp
            )
        }

        // Post text
        if (p.text.isNotEmpty()) {
            Spacer(modifier = Modifier.height(6.dp))
            Text(
                text = p.text,
                fontSize = 14.sp,
                color = MaterialTheme.colorScheme.onBackground,
                maxLines = 4,
                overflow = TextOverflow.Ellipsis
            )
        }

        // Post image
        p.imageUrl?.let { imgUrl ->
            Spacer(modifier = Modifier.height(8.dp))
            AsyncImage(
                model = imgUrl,
                contentDescription = null,
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = 160.dp)
                    .clip(RoundedCornerShape(8.dp))
                    .clickable { onImageClick?.invoke(imgUrl) },
                contentScale = ContentScale.Crop
            )
        }

        // Stats row
        if (p.likeCount > 0 || p.repostCount > 0) {
            Spacer(modifier = Modifier.height(8.dp))
            Row(
                horizontalArrangement = Arrangement.spacedBy(16.dp),
                verticalAlignment = Alignment.CenterVertically
            ) {
                if (p.likeCount > 0) {
                    Row(
                        horizontalArrangement = Arrangement.spacedBy(4.dp),
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Icon(
                            Icons.Default.Favorite,
                            contentDescription = null,
                            modifier = Modifier.size(14.dp),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f)
                        )
                        Text(
                            "${p.likeCount}",
                            fontSize = 12.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f)
                        )
                    }
                }
                if (p.repostCount > 0) {
                    Row(
                        horizontalArrangement = Arrangement.spacedBy(4.dp),
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Icon(
                            Icons.Default.Repeat,
                            contentDescription = null,
                            modifier = Modifier.size(14.dp),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f)
                        )
                        Text(
                            "${p.repostCount}",
                            fontSize = 12.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f)
                        )
                    }
                }
            }
        }
    }
}

private fun fetchBskyPost(handle: String, rkey: String): BskyPost? {
    return try {
        val uri = "at://$handle/app.bsky.feed.post/$rkey"
        val encoded = URLEncoder.encode(uri, "UTF-8")
        val url = URL("https://public.api.bsky.app/xrpc/app.bsky.feed.getPostThread?uri=$encoded&depth=0")
        val conn = url.openConnection().apply {
            connectTimeout = 5000
            readTimeout = 5000
        }
        val text = conn.getInputStream().bufferedReader().readText()
        val json = JSONObject(text)
        val thread = json.optJSONObject("thread") ?: return null
        val post = thread.optJSONObject("post") ?: return null
        val author = post.optJSONObject("author") ?: return null
        val record = post.optJSONObject("record") ?: return null

        // Extract first image if present
        val embed = post.optJSONObject("embed")
        val imageUrl = embed?.optJSONArray("images")
            ?.optJSONObject(0)
            ?.optString("thumb")
            ?.takeIf { it.isNotEmpty() }

        BskyPost(
            authorName = author.optString("displayName", ""),
            authorHandle = author.optString("handle", handle),
            authorAvatar = author.optString("avatar", null),
            text = record.optString("text", ""),
            imageUrl = imageUrl,
            likeCount = post.optInt("likeCount", 0),
            repostCount = post.optInt("repostCount", 0)
        )
    } catch (_: Exception) {
        null
    }
}
