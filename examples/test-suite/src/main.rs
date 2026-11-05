mod harness;
mod offline;
mod online;

use clap::Parser;
use harness::TestSuite;

#[derive(Parser)]
#[command(name = "test-suite", about = "Comprehensive test suite for tslib")]
struct Args {
    /// Only run offline tests
    #[arg(long)]
    offline: bool,

    /// Server address
    #[arg(long, default_value = "192.168.255.252:9987")]
    server: String,

    /// Server password
    #[arg(long, default_value = "fngp")]
    password: String,

    /// Nickname for the test client
    #[arg(long, default_value = "TsLibTest")]
    nickname: String,

    /// Skip tests requiring audio hardware
    #[arg(long)]
    skip_audio_hw: bool,

    /// Enable verbose logging
    #[arg(long)]
    verbose: bool,

    /// Filter tests by name pattern
    #[arg(long)]
    filter: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    if args.verbose {
        tracing_subscriber::fmt()
            .with_env_filter("tslib=debug,test_suite=debug")
            .init();
    }

    println!("tslib test suite");
    println!("{}", "=".repeat(60));

    let mut suite = TestSuite::new();

    // ── Offline tests ────────────────────────────────────────────
    println!("\nRunning offline tests...");

    suite.add_group(offline::test_identity());
    suite.add_group(offline::test_bbcode());
    suite.add_group(offline::test_message_builder());
    suite.add_group(offline::test_chat_history());
    suite.add_group(offline::test_channel_tree());
    suite.add_group(offline::test_opus_codec());
    suite.add_group(offline::test_audio_config());
    suite.add_group(offline::test_bot_config_commands());

    // ── Online tests ─────────────────────────────────────────────
    if !args.offline {
        println!("\nRunning online tests against {}...", args.server);

        let identity = tslib_core::Identity::create().expect("Failed to create identity");
        let config = tslib_core::ClientConfig::builder()
            .address(&args.server)
            .identity(identity)
            .nickname(&args.nickname)
            .password(&args.password)
            .build()
            .expect("Failed to build client config");

        match tslib_core::Client::connect(config) {
            Ok(mut client) => {
                // Wait for connection to be fully established
                match client.wait_connected().await {
                    Ok(()) => {
                        // Process a few rounds of events to let full state populate
                        // (channels, users arrive in subsequent BookEvents)
                        for _ in 0..10 {
                            let _ = client.process_events().await;
                            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        }
                        // Re-sync state after events have been processed
                        let _ = client.sync_state();

                        println!("  Connected to server.");

                        suite.add_group(online::test_connection(&client));
                        suite.add_group(online::test_server_state(&mut client));
                        suite.add_group(online::test_messaging(&mut client).await);
                        suite.add_group(online::test_events(&mut client).await);
                        suite.add_group(online::test_channel_movement(&mut client).await);
                        suite.add_group(online::test_audio(&mut client, args.skip_audio_hw).await);
                        suite.add_group(online::test_bot(&args.server, &args.password).await);

                        let _ = client.disconnect();
                    }
                    Err(e) => {
                        eprintln!("  Failed to establish connection: {}", e);
                        eprintln!("  Skipping all online tests.");
                    }
                }
            }
            Err(e) => {
                eprintln!("  Failed to connect: {}", e);
                eprintln!("  Skipping all online tests.");
            }
        }
    }

    // ── Filter and print ─────────────────────────────────────────
    if let Some(ref pattern) = args.filter {
        suite.filter(pattern);
    }

    suite.print_results();
}
