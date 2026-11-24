use egui::{Color32, ColorImage};
use kinseat_ipc::{Notice, NoticePhase, PreviewFrame};
use std::time::{Duration, Instant};

pub struct TimedNotice {
    pub notice: Notice,
    received: Instant,
}
impl TimedNotice {
    pub fn new(notice: Notice) -> Self {
        Self {
            notice,
            received: Instant::now(),
        }
    }
    pub fn active(&self) -> bool {
        matches!(
            self.notice.phase,
            NoticePhase::Pending
                | NoticePhase::Sending
                | NoticePhase::Displayed
                | NoticePhase::Closing
        )
    }
    pub fn remaining(&self) -> u64 {
        if self.active() {
            self.notice
                .remaining_seconds
                .saturating_sub(self.received.elapsed().as_secs())
        } else {
            0
        }
    }
    pub fn label(&self) -> String {
        let phase = phase_label(&self.notice.phase, self.notice.detail.as_deref());
        if self.active() {
            format!("{} · {phase}", clock(self.remaining()))
        } else {
            phase
        }
    }
    pub fn color(&self) -> Color32 {
        if matches!(self.notice.phase, NoticePhase::Failed)
            || (self.active() && self.remaining() < 60)
        {
            crate::theme::ERR
        } else if self.active() {
            crate::theme::ACCENT
        } else {
            crate::theme::FAINT
        }
    }
}

pub fn clock(seconds: u64) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}
pub fn phase_label(phase: &NoticePhase, detail: Option<&str>) -> String {
    match phase {
        NoticePhase::Pending => "En attente de la session du joueur".into(),
        NoticePhase::Sending => "Envoi".into(),
        NoticePhase::Displayed => "Affiché sur l'écran du joueur".into(),
        NoticePhase::Closing => "Fermeture en cours".into(),
        NoticePhase::Closed => "Fermée".into(),
        NoticePhase::Failed => format!("Échec : {}", detail.unwrap_or("raison inconnue")),
        NoticePhase::Cancelled => "Annulé".into(),
        NoticePhase::Unknown => "Phase inconnue".into(),
    }
}
pub fn validate(minutes: &str, message: &str) -> Result<u32, &'static str> {
    let minutes = minutes
        .trim()
        .parse::<u32>()
        .map_err(|_| "Saisissez un nombre de minutes valide")?;
    if !(1..=120).contains(&minutes) {
        return Err("Choisissez de 1 à 120 minutes");
    }
    if message.trim().is_empty() || message.chars().count() > 220 {
        return Err("Le message doit contenir de 1 à 220 caractères");
    }
    Ok(minutes)
}
/// Messages des opérations sur les places et les membres (création, suppression).
pub fn seat_error(error: &str) -> String {
    match error.split(':').next().unwrap_or("") {
        "conflict" => "Ce membre a déjà une place, la limite de places est atteinte, ou la place est occupée".into(),
        "not_found" => "Compte ou place introuvable : actualisez la liste".into(),
        "invalid_params" => "Nom ou mot de passe refusé (lettres sans accent, chiffres, - ou _ ; mot de passe non vide)".into(),
        "unavailable" => "Le service n’a pas pu terminer l’opération : consultez le journal (Paramètres › Assistance)".into(),
        _ => service_error(error),
    }
}

pub fn service_error(error: &str) -> String {
    let code = error.split(':').next().unwrap_or("");
    match code {
        "conflict" => {
            "Un préavis est déjà en cours sur cette place, ou la place n'est pas prête".into()
        }
        "too_late" => "La fermeture a déjà commencé : annulation impossible".into(),
        "not_found" => "Cette place n'est plus disponible".into(),
        "invalid_params" => "Paramètres refusés par le service".into(),
        _ => format!("Le service n'a pas pu effectuer l'opération : {error}"),
    }
}

// Decode a bounded base64 JPEG without retaining the encoded image or writing to disk.
fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    if input.len() > 4 * 1024 * 1024 {
        return Err("image trop grande".into());
    }
    let mut output = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc = 0_u32;
    let mut bits = 0_u32;
    let mut padded = false;
    for b in input.bytes() {
        if b == b'=' {
            padded = true;
            continue;
        }
        if padded {
            return Err("encodage invalide".into());
        }
        let value = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err("encodage invalide".into()),
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(output)
}
pub fn image_from_frame(frame: &PreviewFrame) -> Result<Option<ColorImage>, String> {
    let Some(encoded) = &frame.jpeg_base64 else {
        return Ok(None);
    };
    let jpeg = decode_base64(encoded)?;
    let decoded = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    if decoded.width() > 4096 || decoded.height() > 4096 {
        return Err("dimensions d'image excessives".into());
    }
    if frame.width != Some(decoded.width()) || frame.height != Some(decoded.height()) {
        return Err("dimensions d'image incohérentes".into());
    }
    Ok(Some(ColorImage::from_rgba_unmultiplied(
        [decoded.width() as usize, decoded.height() as usize],
        decoded.as_raw(),
    )))
}
pub fn demo_image(elapsed: Duration) -> ColorImage {
    let (width, height) = (640_usize, 360_usize);
    let mut pixels = vec![0_u8; width * height * 4];
    let phase = (elapsed.as_millis() / 30 % width as u128) as i32;
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let stripe = ((x as i32 - phase).abs() < 60) && y > 80 && y < 280;
            pixels[i] = if stripe {
                180
            } else {
                38 + (x * 50 / width) as u8
            };
            pixels[i + 1] = if stripe {
                120
            } else {
                48 + (y * 45 / height) as u8
            };
            pixels[i + 2] = if stripe { 65 } else { 58 };
            pixels[i + 3] = 255;
        }
    }
    ColorImage::from_rgba_unmultiplied([width, height], &pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clocks_and_colors() {
        assert_eq!(clock(252), "04:12");
        assert_eq!(clock(60), "01:00");
        let n: Notice = serde_json::from_value(serde_json::json!({"id":"a","seatId":"1","accountName":"Camille","message":"test","minutes":5,"createdAtUtc":"x","deadlineUtc":"x","remainingSeconds":59,"phase":"Displayed"})).unwrap();
        assert_eq!(TimedNotice::new(n).color(), crate::theme::ERR);
    }
    #[test]
    fn validation_and_translations() {
        assert_eq!(validate("120", "ok"), Ok(120));
        assert!(validate("0", "ok").is_err());
        assert!(validate("121", "ok").is_err());
        assert!(validate("5", "").is_err());
        assert!(validate("5", &"é".repeat(221)).is_err());
        assert_eq!(
            phase_label(&NoticePhase::Closing, None),
            "Fermeture en cours"
        );
        assert!(service_error("conflict: déjà").contains("préavis"));
        assert!(service_error("too_late: fermeture").contains("annulation impossible"));
    }
}
