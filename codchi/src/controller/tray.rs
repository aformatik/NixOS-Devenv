use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, AboutMetadata},
    TrayIconBuilder, TrayIconEvent,
};

static LOGO: &'static [u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logo.png"));

fn load_icon(bytes: &[u8]) -> tray_icon::Icon {
    let (icon_rgba, icon_width, icon_height) = {
        let image = image::load_from_memory(bytes)
            .expect("Failed to parse image")
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };
    tray_icon::Icon::from_rgba(icon_rgba, icon_width, icon_height).expect("Failed to open icon")
}

pub fn run() -> anyhow::Result<()> {
    let icon = load_icon(LOGO);
    let event_loop = EventLoopBuilder::new().build();

    let tray_menu = Menu::new();

    let quit_i = MenuItem::new("Quit", true, None);
    tray_menu.append_items(&[
        &PredefinedMenuItem::about(
            None,
            Some(AboutMetadata {
                name: Some("codchi".to_string()),
                copyright: Some("Copyright codchi".to_string()),
                ..Default::default()
            }),
        ),
        &PredefinedMenuItem::separator(),
        &quit_i,
    ])?;
    let mut tray_icon = Some(
        TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("Codchi controller")
            .with_icon(icon)
            .build()
            .unwrap(),
    );

    let menu_channel = MenuEvent::receiver();
    let tray_channel = TrayIconEvent::receiver();

    event_loop.run(move |_event, _, control_flow| {
        *control_flow = ControlFlow::Poll;

        if let Ok(event) = menu_channel.try_recv() {
            if event.id == quit_i.id() {
                tray_icon.take();
                *control_flow = ControlFlow::Exit;
            }
            println!("{event:?}");
        }

        if let Ok(event) = tray_channel.try_recv() {
            println!("{event:?}");
        }
    })
}
