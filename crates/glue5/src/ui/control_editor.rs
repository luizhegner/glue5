//! A transactional editor for gameplay controls, not debug click regions.
use egui::*;
use crate::{settings::{Settings,TouchContext,TouchPosition},touch::Touch};
use super::{shell,theme::{self,palette},widgets};

pub(super) struct Editor {
    preview:Touch,
    draft:Option<Vec<TouchPosition>>,
    scope:TouchContext,
    selected:usize,
}
impl Default for Editor {
    fn default()->Self {Self{preview:Touch::new(),draft:None,scope:TouchContext::OnFoot,selected:0}}
}
pub(super) enum Outcome { Open, Cancel, Save(Vec<TouchPosition>) }

impl Editor {
    pub fn show(&mut self,ctx:&Context,settings:&Settings)->Outcome {
        let output=shell::output_rect(ctx);
        let safe=ctx.content_rect();
        let scale=theme::menu_scale(ctx,safe);
        let bar=44.0*scale;
        let top=Rect::from_min_size(safe.min,vec2(safe.width(),bar));
        let bottom=Rect::from_min_size(pos2(safe.left(),safe.bottom()-bar),vec2(safe.width(),bar));
        let body=Rect::from_min_max(top.left_bottom()+vec2(8.0,8.0),bottom.right_top()-vec2(8.0,8.0));
        let factor=(body.width()/output.width()).min(body.height()/output.height()).max(0.01);
        let canvas=Rect::from_center_size(body.center(),output.size()*factor);
        let game_area=Rect::from_min_max(canvas.min+(safe.min-output.min)*factor,canvas.min+(safe.max-output.min)*factor);
        let draft=self.draft.get_or_insert_with(||settings.touch_positions.clone());
        let mut save=false; let mut cancel=false;
        Area::new(Id::new("control-editor-toolbar")).fixed_pos(top.min).constrain(false).show(ctx,|ui| {
            ui.set_width(top.width()); theme::apply_density(ui,scale);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x=4.0*scale;
                let width=(top.width()-3.0*84.0*scale-16.0*scale).max(44.0*scale);
                ui.allocate_ui(vec2(width,bar),|ui| {
                    widgets::choice(ui,&mut self.scope,&[(TouchContext::OnFoot,"On foot"),(TouchContext::Car,"Car"),
                        (TouchContext::Bicycle,"Bicycle"),(TouchContext::Passenger,"Passenger"),(TouchContext::Aircraft,"Aircraft"),
                        (TouchContext::Submarine,"Submarine"),(TouchContext::Parachute,"Parachute"),(TouchContext::Phone,"Phone")]);
                });
                if widgets::tab_button(ui,"RESET",false,vec2(84.0*scale,bar)).clicked() {
                    draft.retain(|p|p.profile!=settings.touch_layout || p.context!=self.scope);
                }
                save=widgets::tab_button(ui,"SAVE",true,vec2(84.0*scale,bar)).clicked();
                cancel=widgets::tab_button(ui,"CANCEL",false,vec2(84.0*scale,bar)).clicked();
            });
        });
        self.preview.set_profile(settings.touch_layout);
        self.preview.set_steer(settings.touch_steer); self.preview.set_fire(settings.touch_fire);
        self.preview.editor_context(self.scope);
        self.preview.set_positions(draft);
        self.preview.layout(output.size(),safe,settings.touch_size);
        let controls:Vec<_>=self.preview.editable_controls().into_iter().map(|(id,name,at,r)|
            (id,name,canvas.min+(at-output.min)*factor,r*factor)).collect();
        self.selected=self.selected.min(controls.len().saturating_sub(1));
        let painter=ctx.layer_painter(LayerId::new(Order::Middle,Id::new("control-editor-canvas")));
        painter.rect_filled(canvas,0,Color32::from_black_alpha(160));
        painter.rect_stroke(canvas,0,Stroke::new(1.0,palette::LINE),StrokeKind::Inside);
        let layer=LayerId::new(Order::Middle,Id::new("control-editor-game-controls"));
        self.preview.draw(&ctx.layer_painter(layer),1.0);
        ctx.transform_layer_shapes(layer,egui::emath::TSTransform{scaling:factor,translation:canvas.min.to_vec2()-output.min.to_vec2()*factor});
        let mut collisions=0;
        for (i,(id,_,at,radius)) in controls.iter().enumerate() {
            let customized=draft.iter().any(|p|p.profile==settings.touch_layout && p.context==self.scope && &p.id==id);
            let collision=customized && controls.iter().enumerate().any(|(j,(_,_,other,r))|i!=j && at.distance(*other)<(radius+r)*1.05);
            if collision { collisions+=1; }
            if i==self.selected || collision {
                painter.circle_stroke(*at,radius+3.0,Stroke::new(1.5,if collision {Color32::LIGHT_RED}else{palette::TEXT_HI}));
            }
        }
        Area::new(Id::new("control-editor-targets")).fixed_pos(canvas.min).constrain(false).show(ctx,|ui| {
            ui.set_min_size(canvas.size());
            ui.set_clip_rect(canvas);
            for (i,(id,_,at,radius)) in controls.iter().enumerate() {
                let r=ui.interact(Rect::from_center_size(*at,Vec2::splat((radius*2.0).max(44.0))),Id::new(("control-handle",self.scope as u8,id)),Sense::click_and_drag());
                if r.clicked() || r.dragged() {self.selected=i;}
                if r.dragged() {
                    let p=position(draft,settings,self.scope,id,*at,game_area);
                    p.center[0]=(p.center[0]+r.drag_delta().x/game_area.width().max(1.0)).clamp(0.0,1.0);
                    p.center[1]=(p.center[1]+r.drag_delta().y/game_area.height().max(1.0)).clamp(0.0,1.0);
                    ctx.request_repaint();
                }
            }
        });
        Area::new(Id::new("control-editor-properties")).fixed_pos(bottom.min).constrain(false).show(ctx,|ui| {
            ui.set_width(bottom.width()); theme::apply_density(ui,scale);
            ui.horizontal(|ui| {
                if !controls.is_empty() {
                    let options:Vec<_>=controls.iter().enumerate().map(|(i,(_,name,..))|(i,name.as_str())).collect();
                    ui.allocate_ui(vec2((bottom.width()*0.35).max(80.0),bar),|ui|{widgets::choice(ui,&mut self.selected,&options);});
                    let (id,_,at,_)=&controls[self.selected];
                    let mut size=draft.iter().find(|p|p.profile==settings.touch_layout && p.context==self.scope && &p.id==id).map_or(1.0,|p|p.size);
                    ui.spacing_mut().slider_width=(bottom.width()*0.22).max(44.0);
                    if widgets::slider(ui,&mut size,0.75..=1.75,0.05,"×",2).changed() {
                        position(draft,settings,self.scope,id,*at,game_area).size=size;
                    }
                    ui.label(if collisions>0 {"Overlap: move outlined controls apart"}else{"Drag to move · Size applies to the selected control"});
                }
            });
        });
        if cancel {return Outcome::Cancel;}
        if save && collisions==0 {return Outcome::Save(self.draft.take().unwrap_or_default());}
        Outcome::Open
    }
}

fn position<'a>(draft:&'a mut Vec<TouchPosition>,settings:&Settings,scope:TouchContext,id:&str,at:Pos2,area:Rect)->&'a mut TouchPosition {
    let index=if let Some(i)=draft.iter().position(|p|p.profile==settings.touch_layout && p.context==scope && p.id==id) {i}
    else {
        let center=(at-area.min)/area.size();
        draft.push(TouchPosition{version:1,profile:settings.touch_layout,context:scope,id:id.into(),center:[center.x.clamp(0.0,1.0),center.y.clamp(0.0,1.0)],size:1.0});
        draft.len()-1
    };
    &mut draft[index]
}
