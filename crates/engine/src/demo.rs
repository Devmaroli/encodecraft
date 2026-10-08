//! Sample queue used by `ENCODECRAFT_DEMO=1` (screenshots / walkthroughs).

use std::path::PathBuf;

use encodecraft_job::{EnqueueRequest, JobSource, ScreenSize};
use encodecraft_presets::{PresetKind, PresetReason, Quality, INSTALLATION_ID};
use encodecraft_queue::JobStatus;

use crate::{EngineError, Session};

pub fn seed_preset_ui_demo(session: &mut Session) -> Result<(), EngineError> {
    session.queue.jobs.clear();
    session.queue.running = false;
    session.settings.output_dir = PathBuf::from(r"D:\Jobs\Ramadan\out");
    session.settings.recent_preset_ids = vec![
        "system.h264-mp4".into(),
        "user.ramadan-low-4".into(),
    ];

    let mut low = session.presets.get("system.h264-mp4").map_err(|e| EngineError::Message(e.to_string()))?;
    low.id = "user.ramadan-low-4".into();
    low.name = "Ramadan Low 4 Mbps".into();
    low.kind = PresetKind::User;
    low.group = "Ramadan campaign".into();
    low.video.quality = Quality::vbr_2pass(4.0, 4.5);
    low.one_off = false;
    session.presets.upsert_user(low).map_err(|e| EngineError::Message(e.to_string()))?;

    let mut mid = session.presets.get("system.h264-mp4").map_err(|e| EngineError::Message(e.to_string()))?;
    mid.id = "user.ramadan-mid-4".into();
    mid.name = "Ramadan Mid 4 Mbps".into();
    mid.kind = PresetKind::User;
    mid.group = "Ramadan campaign".into();
    mid.video.quality = Quality::vbr_2pass(4.0, 5.0);
    session.presets.upsert_user(mid).map_err(|e| EngineError::Message(e.to_string()))?;

    let project = PathBuf::from(r"D:\Jobs\Ramadan\Ramadan_v3.ecproj");
    let screens: Vec<(&str, &str, u32, u32)> = vec![
        ("Avenues_Ramadan_v3", "The Avenues – Eye of Kuwait", 7560, 2100),
        ("Avenues_Gate_R_Ramadan_v3", "The Avenues Gate – Right side", 6048, 864),
        ("1st_Ring_Road_v3", "1st Ring Road", 1008, 5184),
        ("AL_Nassar_Vertical_v3", "Al Nassar Tower Vertical", 960, 5328),
        ("Grand_Plaza_Entrance", "The Avenues Grand Plaza Entrance", 1920, 1080),
        ("Marina_PalmTrees_v3", "Marina – Palm Trees (pair)", 960, 960),
        ("AL_Salam_Sync", "Al Salam Sync (pair)", 3072, 576),
    ];
    let mut first_id = None;
    for (comp, screen, w, h) in screens {
        let req = EnqueueRequest {
            schema: 1,
            source: JobSource::EffectCraft {
                project: project.clone(),
                comp: Some(comp.into()),
                mezzanine: "prores".into(),
                work_area: false,
                width: Some(w),
                height: Some(h),
            },
            preset_id: "auto".into(),
            preset_override: None,
            output_dir: Some(session.settings.output_dir.clone()),
            naming: Some("{name}_{width}x{height}".into()),
            sizes: vec![ScreenSize::new(screen, w, h)],
            trim: None,
            scale: None,
            start_queue: false,
        };
        let ids = session.queue.enqueue(
            &req,
            &session.presets,
            &session.settings.output_dir,
            "{name}_{width}x{height}",
            true,
            false,
        )?;
        if first_id.is_none() {
            first_id = ids.first().cloned();
        }
    }
    if let Some(j) = session.queue.jobs.iter_mut().find(|j| j.source.display_name().contains("Marina")) {
        if let Ok(inst) = session.presets.get(INSTALLATION_ID) {
            let w = j.preset.video.width;
            let h = j.preset.video.height;
            j.preset = inst;
            j.preset.video.width = w;
            j.preset.video.height = h;
            j.preset_overridden = true;
            j.preset_reason = Some(PresetReason::Manual);
        }
    }
    for (i, j) in session.queue.jobs.iter_mut().enumerate() {
        j.status = match i {
            0 => JobStatus::Done,
            1 => JobStatus::Encoding,
            _ => JobStatus::Waiting,
        };
    }

    let file_req = EnqueueRequest {
        schema: 1,
        source: JobSource::file(r"D:\Jobs\Ramadan\Brand_Sting_4K.mov"),
        preset_id: "system.h264-mp4".into(),
        preset_override: None,
        output_dir: Some(session.settings.output_dir.clone()),
        naming: Some("{name}_{width}x{height}".into()),
        sizes: Vec::new(),
        trim: None,
        scale: None,
        start_queue: false,
    };
    session.queue.enqueue(&file_req, &session.presets, &session.settings.output_dir, "{name}_{width}x{height}", true, false)?;
    if let Some(j) = session.queue.jobs.last_mut() {
        j.preset.video.width = Some(3840);
        j.preset.video.height = Some(2160);
        j.preset_reason = Some(PresetReason::Default);
        let _ = j.apply_resolved_size(3840, 2160, true);
    }

    session.selected = first_id;
    Ok(())
}

/// Seed `n` outputs for virtualization / frame-time benches.
pub fn seed_stress_queue(session: &mut Session, n: usize) -> Result<(), EngineError> {
    session.queue.jobs.clear();
    session.queue.running = false;
    let project = PathBuf::from(r"D:\Jobs\Stress\stress.ecproj");
    let chunk = 64usize;
    let mut left = n;
    let mut g = 0u32;
    while left > 0 {
        let take = left.min(chunk);
        let sizes: Vec<ScreenSize> = (0..take)
            .map(|i| ScreenSize::new(format!("Screen {g}-{}", i + 1), 1920, 1080))
            .collect();
        let req = EnqueueRequest {
            schema: 1,
            source: JobSource::EffectCraft {
                project: project.clone(),
                comp: Some(format!("Comp_{g}")),
                mezzanine: "prores".into(),
                work_area: false,
                width: Some(1920),
                height: Some(1080),
            },
            preset_id: "auto".into(),
            preset_override: None,
            output_dir: Some(PathBuf::from(r"D:\Jobs\Stress\Output")),
            naming: Some("{name}_{width}x{height}".into()),
            sizes,
            trim: None,
            scale: None,
            start_queue: false,
        };
        session.queue.enqueue(&req, &session.presets, &PathBuf::from(r"D:\Jobs\Stress\Output"), "{name}_{width}x{height}", true, false)?;
        left = left.saturating_sub(take);
        g = g.saturating_add(1);
    }
    for j in &mut session.queue.jobs {
        j.status = JobStatus::Waiting;
        j.metadata_pending = false;
    }
    session.selected = session.queue.jobs.first().map(|j| j.id.clone());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths;
    use encodecraft_presets::BETTER_RES_ID;

    #[test]
    fn demo_seed_lists_eye_of_kuwait_as_better_res() {
        let dir = std::env::temp_dir().join(format!("ec-demo-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        paths::set_home(dir);
        let mut session = Session::load().expect("session");
        seed_preset_ui_demo(&mut session).expect("seed");
        assert!(session.queue.jobs.len() >= 8);
        let eye = session
            .queue
            .jobs
            .iter()
            .find(|j| j.screen.as_ref().is_some_and(|s| s.name.to_ascii_lowercase().contains("kuwait")))
            .expect("eye");
        assert_eq!(eye.preset.id, BETTER_RES_ID);
        let file = session.queue.jobs.iter().find(|j| matches!(j.source, JobSource::File { .. })).expect("file");
        assert_eq!(file.preset_reason, Some(PresetReason::Default));
        let groups: std::collections::HashSet<_> = session.queue.jobs.iter().map(|j| j.source.group_key()).collect();
        assert_eq!(groups.len(), 2);
    }
}
