//! Pure queue-selection helpers (marquee, modifiers, remove split).

use egui::{Pos2, Rect};

/// Pointer travel that turns a click into a marquee drag (egui pixels).
pub const MARQUEE_DRAG_PX: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarqueeMode {
    Replace,
    Add,
    Toggle,
}

pub fn range_select(order: &[String], anchor: &str, id: &str) -> Vec<String> {
    match (order.iter().position(|x| x == anchor), order.iter().position(|x| x == id)) {
        (Some(a), Some(b)) => {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            order[lo..=hi].to_vec()
        }
        _ => vec![id.to_string()],
    }
}

pub fn toggle_id(selected: &mut Vec<String>, id: &str) {
    if let Some(i) = selected.iter().position(|x| x == id) {
        selected.remove(i);
    } else {
        selected.push(id.to_string());
    }
}

pub fn marquee_result(hits: &[String], mode: MarqueeMode, base: &[String]) -> Vec<String> {
    match mode {
        MarqueeMode::Replace => hits.to_vec(),
        MarqueeMode::Add => {
            let mut out = base.to_vec();
            for id in hits {
                if !out.iter().any(|x| x == id) {
                    out.push(id.clone());
                }
            }
            out
        }
        MarqueeMode::Toggle => {
            let mut out = base.to_vec();
            for id in hits {
                toggle_id(&mut out, id);
            }
            out
        }
    }
}

pub fn ids_touching_marquee(items: &[(String, Rect)], marquee: Rect) -> Vec<String> {
    items.iter().filter(|(_, r)| r.intersects(marquee)).map(|(id, _)| id.clone()).collect()
}

/// True once the pointer has moved far enough that this is a marquee, not a click.
pub fn marquee_is_drag(start: Pos2, current: Pos2) -> bool {
    (current - start).length() >= MARQUEE_DRAG_PX
}

/// A marquee starts on empty queue space or a row’s non-name columns.
/// `rows` is `(row_rect, name_column_rect)` for each visible row.
pub fn should_begin_marquee(pos: Pos2, rows: &[(Rect, Rect)], scroll: Rect) -> bool {
    for (row, src) in rows {
        if row.contains(pos) {
            return !src.contains(pos);
        }
    }
    scroll.contains(pos)
}

pub fn split_encoding<'a>(jobs: &'a [encodecraft_queue::Job], ids: &[String]) -> (Vec<String>, Vec<String>) {
    let mut encoding = Vec::new();
    let mut rest = Vec::new();
    for id in ids {
        match jobs.iter().find(|j| j.id == *id) {
            Some(j) if j.status.is_active() => encoding.push(id.clone()),
            Some(_) => rest.push(id.clone()),
            None => {}
        }
    }
    (encoding, rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use encodecraft_job::JobSource;
    use encodecraft_presets::PresetLibrary;
    use encodecraft_queue::{EncodeQueue, JobStatus};
    use std::path::PathBuf;

    fn order() -> Vec<String> {
        vec!["a".into(), "b".into(), "c".into(), "d".into()]
    }

    #[test]
    fn shift_selects_inclusive_range() {
        assert_eq!(range_select(&order(), "b", "d"), vec!["b", "c", "d"]);
        assert_eq!(range_select(&order(), "d", "a"), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn ctrl_toggles_membership() {
        let mut s = vec!["a".into()];
        toggle_id(&mut s, "c");
        assert_eq!(s, vec!["a", "c"]);
        toggle_id(&mut s, "a");
        assert_eq!(s, vec!["c"]);
    }

    #[test]
    fn marquee_replace_add_toggle() {
        let hits = vec!["b".into(), "c".into()];
        let base = vec!["a".into(), "c".into()];
        assert_eq!(marquee_result(&hits, MarqueeMode::Replace, &base), vec!["b", "c"]);
        assert_eq!(marquee_result(&hits, MarqueeMode::Add, &base), vec!["a", "c", "b"]);
        let toggled = marquee_result(&hits, MarqueeMode::Toggle, &base);
        assert!(toggled.contains(&"a".to_string()));
        assert!(toggled.contains(&"b".to_string()));
        assert!(!toggled.contains(&"c".to_string()));
    }

    #[test]
    fn marquee_hit_tests_touching_rects() {
        let items = vec![
            ("a".into(), Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 20.0))),
            ("b".into(), Rect::from_min_max(egui::pos2(0.0, 20.0), egui::pos2(100.0, 40.0))),
            ("c".into(), Rect::from_min_max(egui::pos2(0.0, 40.0), egui::pos2(100.0, 60.0))),
        ];
        let band = Rect::from_min_max(egui::pos2(10.0, 15.0), egui::pos2(40.0, 45.0));
        assert_eq!(ids_touching_marquee(&items, band), vec!["a", "b", "c"]);
    }

    #[test]
    fn hit_testing_name_column_vs_blank_and_empty() {
        let row = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 36.0));
        let src = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(160.0, 36.0));
        let scroll = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 400.0));
        let rows = [(row, src)];
        assert!(!should_begin_marquee(egui::pos2(40.0, 18.0), &rows, scroll), "name column keeps reorder");
        assert!(should_begin_marquee(egui::pos2(300.0, 18.0), &rows, scroll), "status/blank column marquees");
        assert!(should_begin_marquee(egui::pos2(80.0, 200.0), &rows, scroll), "empty scroll space marquees");
        assert!(!should_begin_marquee(egui::pos2(900.0, 18.0), &rows, scroll), "outside the queue does not");
        assert!(!marquee_is_drag(egui::pos2(300.0, 18.0), egui::pos2(301.0, 18.0)));
        assert!(marquee_is_drag(egui::pos2(300.0, 18.0), egui::pos2(300.0, 40.0)));
    }

    #[test]
    fn split_encoding_separates_active_jobs() {
        let lib = PresetLibrary::new();
        let mut q = EncodeQueue::new();
        let mut req = encodecraft_job::EnqueueRequest {
            schema: 1,
            source: JobSource::file("/ads/a.mp4"),
            preset_id: "system.h264-mp4".into(),
            preset_override: None,
            output_dir: Some(PathBuf::from("/out")),
            naming: Some("{name}".into()),
            sizes: vec![],
            trim: None,
            scale: None,
            start_queue: false,
        };
        q.enqueue(&req, &lib, std::path::Path::new("/out"), "{name}", false, true).unwrap();
        req.source = JobSource::file("/ads/b.mp4");
        q.enqueue(&req, &lib, std::path::Path::new("/out"), "{name}", false, true).unwrap();
        q.jobs[0].status = JobStatus::Encoding;
        let ids = q.job_ids();
        let (enc, rest) = split_encoding(&q.jobs, &ids);
        assert_eq!(enc, vec![ids[0].clone()]);
        assert_eq!(rest, vec![ids[1].clone()]);
    }
}
