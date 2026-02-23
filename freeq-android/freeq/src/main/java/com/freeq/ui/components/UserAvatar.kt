package com.freeq.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.freeq.ui.theme.FreeqColors
import kotlin.math.abs

private val avatarColors = listOf(
    Color(0xFF6C63FF),
    Color(0xFF43B581),
    Color(0xFFFAA61A),
    Color(0xFFF04747),
    Color(0xFFE91E8C),
    Color(0xFF1ABC9C),
    Color(0xFFE67E22),
    Color(0xFF3498DB),
)

@Composable
fun UserAvatar(
    nick: String,
    size: Dp = 40.dp,
    modifier: Modifier = Modifier
) {
    val initial = nick.firstOrNull()?.uppercaseChar() ?: '?'
    val hash = nick.fold(0) { acc, c -> acc + c.code }
    val bgColor = avatarColors[abs(hash) % avatarColors.size]
    val fontSize = (size.value * 0.4f).sp

    Box(
        modifier = modifier
            .size(size)
            .clip(CircleShape)
            .background(bgColor.copy(alpha = 0.85f)),
        contentAlignment = Alignment.Center
    ) {
        Text(
            text = initial.toString(),
            color = Color.White,
            fontSize = fontSize,
            fontWeight = FontWeight.Bold
        )
    }
}
