//! Non-modal, fixed diagnostic ownership strip. No animation or game assets.
use egui::{Align2,Color32,Context,FontId,Id,LayerId,Order,Rect,Stroke,Vec2,pos2,vec2};
pub const NOTICE:&str="A IA está controlando o jogo";
pub fn stop_rect(safe:Rect)->Rect {
    Rect::from_min_size(pos2(safe.right()-104.0,safe.top()+4.0),vec2(100.0,44.0))
}
pub fn banner_rect(safe:Rect)->Rect {
    let stop=stop_rect(safe);
    let start=safe.left()+safe.width()*0.25;
    let end=stop.left()-8.0;
    let width=(end-start).min(340.0).max(1.0);
    let left=(safe.center().x-width*0.5).clamp(start,(end-width).max(start));
    Rect::from_min_size(pos2(left,safe.top()+4.0),vec2(width,44.0))
}
pub fn draw(ctx:&Context,safe:Rect)->Rect {
    let p=ctx.layer_painter(LayerId::new(Order::Tooltip,Id::new("diagnostic-control-banner")));
    let banner=banner_rect(safe);
    p.rect_filled(banner,2.0,Color32::from_black_alpha(186));
    let text=p.layout(NOTICE.into(),FontId::proportional(13.0),Color32::WHITE,(banner.width()-12.0).max(1.0));
    p.galley(banner.center()-text.size()*0.5,text,Color32::WHITE);
    let stop=stop_rect(safe);
    p.rect_filled(stop,2.0,Color32::from_black_alpha(225));
    p.rect_stroke(stop,2.0,Stroke::new(1.0,Color32::from_white_alpha(180)),egui::StrokeKind::Inside);
    p.text(stop.center(),Align2::CENTER_CENTER,"Parar IA",FontId::proportional(14.0),Color32::WHITE);
    banner.union(stop)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn strip_fits_and_leaves_radar_left_side_clear(){
        for size in [vec2(960.0,432.0),vec2(844.0,390.0),vec2(390.0,844.0)] {
            let safe=Rect::from_min_size(pos2(24.0,12.0),size-Vec2::splat(48.0));
            assert!(safe.contains_rect(stop_rect(safe)));assert!(safe.contains_rect(banner_rect(safe)));
            assert!(!stop_rect(safe).intersects(banner_rect(safe)));
            assert!(banner_rect(safe).left()>=safe.left()+safe.width()*0.25);
            assert!(stop_rect(safe).height()>=44.0);
        }
    }
}
