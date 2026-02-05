pub fn render_progress_bar(percent: f32, width: usize) -> String {
    let filled = ((percent / 100.0) * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);
    // Solid block for filled, dash for empty
    format!("{}{}", "█".repeat(filled), "-".repeat(empty))
}
