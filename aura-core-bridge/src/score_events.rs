use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ScoreEventKind { Chord, Lyric, Expression, Tuplet, Ornament }
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScoreEvent { pub kind: ScoreEventKind, pub tick: u64, pub text: String, pub voice: u8 }
impl ScoreEvent { pub fn validate(&self)->bool { self.text.len()<=1024&&!self.text.trim().is_empty()&&self.voice<16 } }
pub fn validate_score(events:&[ScoreEvent])->bool { events.len()<=1_000_000&&events.iter().all(ScoreEvent::validate)&&events.windows(2).all(|w|w[0].tick<=w[1].tick) }
pub fn events_for_voice(events: &[ScoreEvent], voice: u8) -> Vec<ScoreEvent> { if voice >= 16 { return Vec::new(); } events.iter().filter(|event| event.voice == voice).cloned().collect() }
pub fn render_lead_sheet(events: &[ScoreEvent]) -> Option<String> {
    if !validate_score(events) { return None; }
    let mut out = String::new();
    for event in events { let kind = match event.kind { ScoreEventKind::Chord => "CHORD", ScoreEventKind::Lyric => "LYRIC", ScoreEventKind::Expression => "EXPR", ScoreEventKind::Tuplet => "TUPLET", ScoreEventKind::Ornament => "ORNAMENT" }; out.push_str(&format!("{}\t{}\t{}\t{}\n", event.tick, event.voice, kind, event.text)); }
    Some(out)
}
pub fn export_musicxml(events: &[ScoreEvent], title: &str) -> Option<String> {
    if title.trim().is_empty() || title.len() > 256 || !validate_score(events) { return None; }
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let mut xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><score-partwise version=\"3.1\"><work><work-title>{}</work-title></work><part-list><score-part id=\"P1\"><part-name> Aura </part-name></score-part></part-list><part id=\"P1\"><measure number=\"1\">", esc(title));
    for event in events { let tag = match event.kind { ScoreEventKind::Chord => "harmony", ScoreEventKind::Lyric => "lyric", ScoreEventKind::Expression => "direction", ScoreEventKind::Tuplet => "time-modification", ScoreEventKind::Ornament => "ornaments" }; xml.push_str(&format!("<{}><text>{}</text><tick>{}</tick><voice>{}</voice></{}>", tag, esc(&event.text), event.tick, event.voice, tag)); }
    xml.push_str("</measure></part></score-partwise>"); Some(xml)
}

/// Imports the compact MusicXML representation emitted by `export_musicxml`.
pub fn import_musicxml(xml: &str) -> Option<Vec<ScoreEvent>> {
    if xml.len() > 16 * 1024 * 1024 || !xml.contains("<score-partwise") { return None; }
    let mut out = Vec::new();
    for kind in [ScoreEventKind::Chord, ScoreEventKind::Lyric, ScoreEventKind::Expression, ScoreEventKind::Tuplet, ScoreEventKind::Ornament] {
        let tag = match kind { ScoreEventKind::Chord=>"harmony", ScoreEventKind::Lyric=>"lyric", ScoreEventKind::Expression=>"direction", ScoreEventKind::Tuplet=>"time-modification", ScoreEventKind::Ornament=>"ornaments" };
        let mut rest = xml;
        while let Some(start) = rest.find(&format!("<{tag}>")) { rest = &rest[start + tag.len() + 2..]; let end = rest.find(&format!("</{tag}>"))?; let block=&rest[..end]; let text=block.strip_prefix("<text>")?.split("</text>").next()?; let tick=block.split("<tick>").nth(1)?.split("</tick>").next()?.parse().ok()?; let voice=block.split("<voice>").nth(1)?.split("</voice>").next()?.parse().ok()?; out.push(ScoreEvent{kind:kind.clone(),tick,text: text.replace("&lt;","<").replace("&gt;",">").replace("&amp;","&"),voice}); rest=&rest[end+tag.len()+3..]; }
    }
    out.sort_by_key(|e| e.tick); validate_score(&out).then_some(out)
}

#[cfg(test)]
mod tests { use super::*; #[test] fn accepts_ordered_notation() { let e=ScoreEvent{kind:ScoreEventKind::Chord,tick:0,text:"Cmaj7".into(),voice:0}; assert!(validate_score(&[e])); } #[test] fn musicxml_roundtrip() { let events=vec![ScoreEvent{kind:ScoreEventKind::Lyric,tick:12,text:"A&B".into(),voice:1}]; let xml=export_musicxml(&events,"Song").unwrap(); let parsed=import_musicxml(&xml).unwrap(); assert_eq!(parsed,events); } }
