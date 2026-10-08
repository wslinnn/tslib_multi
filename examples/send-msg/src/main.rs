use tslib_core::{ClientConfig, Client, Identity};
use tslib_core::events::Event;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("tslib_core=debug,send_msg=debug")
        .init();

    let identity = Identity::create().expect("Failed to create identity");
    let config = ClientConfig::builder()
        .address("192.168.255.252:9987")
        .identity(identity)
        .nickname("ClaudeBot")
        .password("fngp")
        .build()
        .expect("Failed to build config");

    let mut client = Client::connect(config).expect("Failed to connect");
    client.wait_connected().await.expect("Failed to wait for connection");

    for _ in 0..10 {
        let _ = client.process_events().await;
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }
    let _ = client.sync_state();

    // Move to flamme-demon's channel
    let state = client.server_state();
    if let Some(user) = state.find_user_by_name("flamme-demon") {
        let channel_id = user.channel_id;
        println!("Moving to flamme-demon's channel (id={})", channel_id);
        let _ = client.move_to_channel(channel_id);
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        let _ = client.process_events().await;
    }

    // Send greeting
    if let Some(user) = client.server_state().find_user_by_name("flamme-demon") {
        client.send_private_message(user.id, "Salut ! La reception des messages est corrigee, envoie moi un MP et un message dans le canal !").expect("send failed");
        println!("Greeting sent!");
    }

    // Listen 30s
    println!("=== Listening 30s - send me messages! ===");
    let start = tokio::time::Instant::now();
    while start.elapsed() < tokio::time::Duration::from_secs(30) {
        match client.process_events().await {
            Ok(events) => {
                for event in events {
                    match &event {
                        Event::TextMessage { sender_name, message, target, .. } => {
                            println!(">>> [MSG {:?}] {}: {}", target, sender_name, message);
                        }
                        Event::Poked { poker_name, message, .. } => {
                            println!(">>> [POKE] {}: {}", poker_name, message);
                        }
                        Event::AudioReceived { .. } => {
                            // silent - too noisy
                        }
                        Event::TalkStatusStart { user_id, .. } => {
                            println!("[TALK START] id={}", user_id);
                        }
                        Event::TalkStatusStop { user_id } => {
                            println!("[TALK STOP] id={}", user_id);
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => eprintln!("Error: {}", e),
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    let _ = client.disconnect();
    println!("Done.");
}
