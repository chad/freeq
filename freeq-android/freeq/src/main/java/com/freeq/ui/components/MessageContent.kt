package com.freeq.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Link
import androidx.compose.material.icons.filled.PlayArrow
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
import java.net.URI

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

    // Priority: image > YouTube > generic link
    val imageUrl = IMAGE_PATTERN.find(text)?.value ?: CDN_PATTERN.find(text)?.value
    val ytMatch = if (imageUrl == null) YOUTUBE_PATTERN.find(text) else null
    val linkUrl = if (imageUrl == null && ytMatch == null) URL_PATTERN.find(text)?.value else null

    val embedUrl = imageUrl ?: ytMatch?.let { URL_PATTERN.find(text)?.value } ?: linkUrl
    val remainingText = embedUrl?.let { text.replace(it, "").trim() } ?: text

    // Text portion
    val showText = if (embedUrl != null) remainingText else text
    if (showText.isNotEmpty()) {
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
