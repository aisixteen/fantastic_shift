use fantastic_team_app::{start_args, target_from_args, web_url, Target};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let target = match target_from_args(&args) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("fantastic-team-app: {e}");
            std::process::exit(2);
        }
    };
    let url = web_url(&target);
    if let Target::Start { binary, port } = &target {
        println!(
            "fantastic-team-app: start {binary} {}",
            start_args(*port).join(" ")
        );
    }
    println!("fantastic-team-app: web UI at {url}");
    #[cfg(feature = "tauri")]
    open_window(&url);
}

#[cfg(feature = "tauri")]
fn open_window(url: &str) {
    let url: tauri::Url = url.parse().expect("valid URL");
    tauri::Builder::default()
        .setup(move |app| {
            tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(url))
                .title("Fantastic Team")
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!("../../app/tauri.conf.json"))
        .expect("tauri app failed");
}
