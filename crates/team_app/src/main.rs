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
}
