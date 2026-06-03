//! 系统托盘图标。交互菜单由自定义 WebPanel 负责。

use anyhow::{anyhow, Result};
use tray_icon::Icon;

pub fn load_icon() -> Result<Icon> {
    const S: u32 = 44;
    let mut rgba = Vec::with_capacity((S * S * 4) as usize);
    for y in 0..S {
        for x in 0..S {
            let p = Point {
                x: x as f32 + 0.5,
                y: y as f32 + 0.5,
            };
            let mut alpha: f32 = 0.0;
            alpha = alpha.max(rounded_rect_stroke(p, 13.0, 9.0, 18.0, 22.0, 5.5, 2.7));
            alpha = alpha.max(rounded_rect_stroke(p, 19.0, 13.0, 18.0, 22.0, 5.5, 2.7));
            alpha = alpha.max(line_stroke(p, 17.0, 20.0, 27.0, 20.0, 2.4));
            alpha = alpha.max(line_stroke(p, 17.0, 25.0, 27.0, 25.0, 2.4));
            alpha = alpha.max(line_stroke(p, 20.0, 30.0, 24.0, 30.0, 2.4));
            let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
            rgba.extend_from_slice(&[0, 0, 0, a]);
        }
    }
    Icon::from_rgba(rgba, S, S).map_err(|e| anyhow!("Icon::from_rgba: {e}"))
}

#[derive(Clone, Copy)]
struct Point {
    x: f32,
    y: f32,
}

fn smooth_alpha(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

fn rounded_rect_stroke(p: Point, x: f32, y: f32, w: f32, h: f32, r: f32, stroke: f32) -> f32 {
    let cx = (p.x - (x + w / 2.0)).abs() - (w / 2.0 - r);
    let cy = (p.y - (y + h / 2.0)).abs() - (h / 2.0 - r);
    let ox = cx.max(0.0);
    let oy = cy.max(0.0);
    let outside = (ox * ox + oy * oy).sqrt() + cx.max(cy).min(0.0) - r;
    let d = outside.abs() - stroke / 2.0;
    smooth_alpha(d)
}

fn line_stroke(p: Point, x1: f32, y1: f32, x2: f32, y2: f32, stroke: f32) -> f32 {
    let vx = x2 - x1;
    let vy = y2 - y1;
    let len2 = vx * vx + vy * vy;
    let t = (((p.x - x1) * vx + (p.y - y1) * vy) / len2).clamp(0.0, 1.0);
    let px = x1 + vx * t;
    let py = y1 + vy * t;
    let dx = p.x - px;
    let dy = p.y - py;
    smooth_alpha((dx * dx + dy * dy).sqrt() - stroke / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn icon_ok() {
        load_icon().expect("icon");
    }
}
