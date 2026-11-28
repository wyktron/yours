//! The red skull by a breached sender: shown when the row carries breach
//! details, hidden when it does not, and its click opens the breach popup.

use i_slint_core::accessibility::AccessibleStringProperty;
use i_slint_core::item_tree::ItemRc;
use i_slint_core::items::AccessibleRole;
use i_slint_core::window::WindowInner;
use slint::platform::{
    Platform, PointerEventButton, WindowAdapter, WindowEvent,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};
use slint::{ComponentHandle, Model, ModelRc, Rgb8Pixel, VecModel};
use std::{ops::ControlFlow, rc::Rc};

slint::slint! {
    #[edition = "1.19"]
    import { EmailRow, ListAttachment, MailLabelRow, MailListEntry, SenderBreach } from "../ui/models.slint";
    import { MailListPane } from "../ui/views/mail-list-pane.slint";
    import { SenderBreaches } from "../ui/sender-breaches.slint";
    export { SenderBreaches }
    import { BreachDialog } from "../ui/dialogs/breach-dialog.slint";
    import { MotionSettings } from "../ui/design-system.slint";

    export component BreachListHarness inherits Window {
        preferred-width: 397px;
        preferred-height: 260px;
        in-out property <[EmailRow]> emails: [];
        in-out property <[MailListEntry]> entries: [];
        init => { MotionSettings.enabled = false; }

        MailListPane {
            width: parent.width;
            height: parent.height;
            emails: root.emails;
            list_entries: root.entries;
            surface: #18191a;
            heading: white;
            body-text: white;
            secondary-text: #cccccc;
            muted-text: #999999;
            border: #3a3b3c;
            border-subtle: #303030;
            accent: #5ca3ff;
        }
        if SenderBreaches.visible : BreachDialog {
            x: 0px;
            y: 0px;
            width: parent.width;
            height: parent.height;
        }
    }
}

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn breach(title: &str) -> SenderBreach {
    // HIBP-shaped sample values for the fixture render.
    let (domain, date, records, classes) = match title {
        "LinkedIn" => ("linkedin.com", "2012-05-05", 117_430_911, "Email addresses, Passwords"),
        "Dropbox" => ("dropbox.com", "2012-07-01", 68_648_009, "Email addresses, Passwords"),
        _ => ("adobe.com", "2013-10-04", 152_445_165, "Email addresses, Password hints"),
    };
    SenderBreach {
        name: title.into(),
        title: title.into(),
        domain: domain.into(),
        date: date.into(),
        data_classes: classes.into(),
        records,
    }
}

fn list_with(breaches: Vec<SenderBreach>) -> (BreachListHarness, Rc<MinimalSoftwareWindow>) {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
    let ui = BreachListHarness::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(397, 260));
    let row = EmailRow {
        id: 1,
        sender: "Sender".into(),
        address: "sender@example.com".into(),
        subject: "Subject".into(),
        preview: "Preview".into(),
        breaches: ModelRc::new(VecModel::from(breaches)),
        ..Default::default()
    };
    let entry = MailListEntry {
        show_row: true,
        email_index: 0,
        email: row.clone(),
        ..Default::default()
    };
    ui.set_emails(ModelRc::new(VecModel::from(vec![row])));
    ui.set_entries(ModelRc::new(VecModel::from(vec![entry])));
    ui.show().unwrap();
    (ui, window)
}

/// The shown element with this accessible label, of `role` if given.
fn labelled(window: &slint::Window, label: &str, role: Option<AccessibleRole>) -> Option<ItemRc> {
    let inner = WindowInner::from_pub(window);
    ItemRc::new_root(inner.component()).visit_descendants(|item| {
        if item.is_visible()
            && role.is_none_or(|role| item.accessible_role() == role)
            && item
                .accessible_string_property(AccessibleStringProperty::Label)
                .as_deref()
                == Some(label)
        {
            ControlFlow::Break(item.clone())
        } else {
            ControlFlow::Continue(())
        }
    })
}

fn click(window: &slint::Window, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

#[test]
fn a_breached_sender_shows_a_skull_with_the_breach_count() {
    let (ui, window) = list_with(vec![breach("Adobe"), breach("LinkedIn"), breach("Dropbox")]);
    slint::platform::update_timers_and_animations();
    let skull = labelled(&window, "3 known breaches", Some(AccessibleRole::Button))
        .expect("the skull names the breach count");
    let geometry = skull.geometry();
    let origin = skull.map_to_window(geometry.origin);
    click(&window, origin.x + geometry.width() / 2.0, origin.y + geometry.height() / 2.0);
    slint::platform::update_timers_and_animations();
    let popup = ui.global::<SenderBreaches>();
    assert!(popup.get_visible(), "clicking the skull opens the breach popup");
    assert_eq!(popup.get_address(), "sender@example.com");
    assert_eq!(popup.get_breaches().row_count(), 3);
}

#[test]
fn a_clean_sender_shows_no_skull() {
    let (_ui, window) = list_with(Vec::new());
    slint::platform::update_timers_and_animations();
    assert!(
        labelled(&window, "3 known breaches", Some(AccessibleRole::Button)).is_none(),
        "no skull is shown without breach details"
    );
    assert!(labelled(&window, "1 known breaches", Some(AccessibleRole::Button)).is_none());
}

/// Paint the harness window into an RGB buffer (physical pixels).
fn capture(window: &MinimalSoftwareWindow) -> (usize, usize, Vec<Rgb8Pixel>) {
    let size = window.size();
    let (width, height) = (size.width as usize, size.height as usize);
    let mut pixels = vec![Rgb8Pixel::default(); width * height];
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        std::thread::sleep(std::time::Duration::from_millis(120));
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, width);
        });
    }
    (width, height, pixels)
}

/// A 24-bit BMP, so the fixture render needs no image dependency.
fn write_bmp(path: &std::path::Path, width: usize, height: usize, pixels: &[Rgb8Pixel]) {
    let row = (width * 3).div_ceil(4) * 4;
    let data = row * height;
    let mut out = Vec::with_capacity(54 + data);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + data) as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(data as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..height).rev() {
        for x in 0..width {
            let pixel = pixels[y * width + x];
            out.extend_from_slice(&[pixel.b, pixel.g, pixel.r]);
        }
        out.extend(std::iter::repeat_n(0u8, row - width * 3));
    }
    std::fs::write(path, out).expect("write screenshot");
}

/// Offscreen fixture render for the report: one row with three sample
/// breaches (skull shown) and the popup open. This is FIXTURE data, not a
/// live Have I Been Pwned answer: no API key was available, so the live
/// lookup is unverified (HIBP answers HTTP 401 without a paid key).
#[test]
#[ignore]
fn fixture_screenshot_list_and_popup() {
    let (ui, window) = list_with(vec![breach("Adobe"), breach("LinkedIn"), breach("Dropbox")]);
    ui.window().set_size(slint::PhysicalSize::new(760, 520));
    let popup = ui.global::<SenderBreaches>();
    popup.set_address("fixture.sender@example.com".into());
    popup.set_breaches(ModelRc::new(VecModel::from(vec![
        breach("Adobe"),
        breach("LinkedIn"),
        breach("Dropbox"),
    ])));
    slint::platform::update_timers_and_animations();
    let out = std::path::PathBuf::from(
        std::env::var("BREACH_FIXTURE_OUT").unwrap_or_else(|_| "/tmp/breach-shots".into()),
    );
    std::fs::create_dir_all(&out).unwrap();
    let (width, height, pixels) = capture(&window);
    write_bmp(&out.join("fixture-mail-list-skull.bmp"), width, height, &pixels);
    ui.global::<SenderBreaches>().set_visible(true);
    slint::platform::update_timers_and_animations();
    let (width, height, pixels) = capture(&window);
    write_bmp(&out.join("fixture-breach-popup.bmp"), width, height, &pixels);
    eprintln!("fixture screenshots in {}", out.display());
}
