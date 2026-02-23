package com.freeq.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil.compose.SubcomposeAsyncImage

private val IMAGE_PATTERN = Regex(
    """https?://\S+\.(?:png|jpg|jpeg|gif|webp)(?:\?\S*)?""",
    RegexOption.IGNORE_CASE
)
private val CDN_PATTERN = Regex(
    """https?://cdn\.bsky\.app/img/[^\s<]+""",
    RegexOption.IGNORE_CASE
)
private val URL_PATTERN = Regex(
    """https?://\S+"""
)

/**
 * Renders message text with inline image embeds.
 * Detects image URLs and AT Protocol CDN URLs, rendering them as inline images.
 * Non-image URLs are left as plain text for now.
 */
@Composable
fun MessageContent(
    text: String,
    isAction: Boolean,
    fromNick: String,
    onImageClick: ((String) -> Unit)? = null
) {
    val imageUrl = IMAGE_PATTERN.find(text)?.value ?: CDN_PATTERN.find(text)?.value

    if (imageUrl != null) {
        // Show text without the image URL
        val remainingText = text.replace(imageUrl, "").trim()
        if (remainingText.isNotEmpty()) {
            if (isAction) {
                Text(
                    text = "$fromNick $remainingText",
                    fontSize = 15.sp,
                    fontStyle = FontStyle.Italic,
                    color = MaterialTheme.colorScheme.onBackground
                )
            } else {
                Text(
                    text = remainingText,
                    fontSize = 15.sp,
                    color = MaterialTheme.colorScheme.onBackground
                )
            }
        }

        // Inline image
        InlineImage(url = imageUrl, onClick = { onImageClick?.invoke(imageUrl) })
    } else {
        // Plain text (no image URL)
        if (isAction) {
            Text(
                text = "$fromNick $text",
                fontSize = 15.sp,
                fontStyle = FontStyle.Italic,
                color = MaterialTheme.colorScheme.onBackground
            )
        } else {
            Text(
                text = text,
                fontSize = 15.sp,
                color = MaterialTheme.colorScheme.onBackground
            )
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
            // Fall back to showing the URL as text
            Text(
                text = url,
                fontSize = 15.sp,
                color = MaterialTheme.colorScheme.primary
            )
        }
    )
}
