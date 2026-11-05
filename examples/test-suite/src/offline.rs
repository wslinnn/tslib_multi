use crate::harness::TestGroup;

use tslib_core::Identity;

use tslib_chat::bbcode::{self, BBCodeParser};
use tslib_chat::history::ChatHistory;
use tslib_chat::message::{ChatMessage, MessageBuilder, MessageTarget};

use tslib_channel::ChannelTree;
use tslib_core::state::Channel;

use tslib_audio::{AudioConfig, OpusCodec};
use tslib_audio::codec::{Decoder, Encoder};

use tslib_bot::command::{Command, CommandRegistry, FnHandler};
use tslib_bot::BotConfig;

// ── Helpers ──────────────────────────────────────────────────────────

fn make_channel(id: u64, parent_id: u64, name: &str, order: u64) -> Channel {
    Channel {
        id,
        parent_id,
        name: name.to_string(),
        topic: None,
        description: None,
        order,
        is_permanent: true,
        is_semi_permanent: false,
        is_default: id == 1,
        has_password: false,
        codec: 4,
        codec_quality: 7,
        max_clients: -1,
        max_family_clients: -1,
        needed_talk_power: 0,
        icon_id: 0,
        is_subscribed: true,
        permission_hints: 0,
    }
}

// ── 1. Identity ──────────────────────────────────────────────────────

pub fn test_identity() -> TestGroup {
    let mut g = TestGroup::new("Identity");

    g.run_test("identity_create", || {
        let id = Identity::create().map_err(|e| e.to_string())?;
        if id.unique_id().is_empty() {
            return Err("unique_id is empty".into());
        }
        Ok(())
    });

    g.run_test("identity_unique_id_format", || {
        let id = Identity::create().map_err(|e| e.to_string())?;
        let uid = id.unique_id();
        // UID should be valid base64
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(&uid)
            .map_err(|e| format!("UID '{}' is not valid base64: {}", uid, e))?;
        Ok(())
    });

    g.run_test("identity_export_import", || {
        let id = Identity::create().map_err(|e| e.to_string())?;
        let exported = id.export_string().map_err(|e| e.to_string())?;
        let imported = Identity::from_string(&exported).map_err(|e| e.to_string())?;
        if id.unique_id() != imported.unique_id() {
            return Err(format!(
                "UID mismatch: {} vs {}",
                id.unique_id(),
                imported.unique_id()
            ));
        }
        Ok(())
    });

    g.run_test("identity_save_load", || {
        let id = Identity::create().map_err(|e| e.to_string())?;
        let tmp = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
        id.save(tmp.path()).map_err(|e| e.to_string())?;
        let loaded = Identity::load(tmp.path()).map_err(|e| e.to_string())?;
        if id.unique_id() != loaded.unique_id() {
            return Err("UID mismatch after save/load".into());
        }
        Ok(())
    });

    g.run_test("identity_set_nickname", || {
        let mut id = Identity::create().map_err(|e| e.to_string())?;
        id.set_nickname("test");
        match id.nickname() {
            Some("test") => Ok(()),
            other => Err(format!("Expected Some(\"test\"), got {:?}", other)),
        }
    });

    g.run_test("identity_two_distinct", || {
        let id1 = Identity::create().map_err(|e| e.to_string())?;
        let id2 = Identity::create().map_err(|e| e.to_string())?;
        if id1.unique_id() == id2.unique_id() {
            return Err("Two identities have the same UID".into());
        }
        Ok(())
    });

    g
}

// ── 2. BBCode ────────────────────────────────────────────────────────

pub fn test_bbcode() -> TestGroup {
    let mut g = TestGroup::new("BBCode");

    g.run_test("bbcode_bold_html", || {
        let result = BBCodeParser::to_html("[b]test[/b]");
        if result != "<strong>test</strong>" {
            return Err(format!("Got: {}", result));
        }
        Ok(())
    });

    g.run_test("bbcode_italic_html", || {
        let result = BBCodeParser::to_html("[i]test[/i]");
        if result != "<em>test</em>" {
            return Err(format!("Got: {}", result));
        }
        Ok(())
    });

    g.run_test("bbcode_color_html", || {
        let result = BBCodeParser::to_html("[color=red]text[/color]");
        if result != "<span style=\"color: red\">text</span>" {
            return Err(format!("Got: {}", result));
        }
        Ok(())
    });

    g.run_test("bbcode_url_html", || {
        let result = BBCodeParser::to_html("[url=http://x.com]Link[/url]");
        if !result.contains("<a href=") {
            return Err(format!("Missing <a href=: {}", result));
        }
        Ok(())
    });

    g.run_test("bbcode_nested_html", || {
        let result = BBCodeParser::to_html("[b][i]text[/i][/b]");
        if !result.contains("<strong>") || !result.contains("<em>") {
            return Err(format!("Missing tags: {}", result));
        }
        Ok(())
    });

    g.run_test("bbcode_to_plain", || {
        let result = BBCodeParser::to_plain("[b]hello[/b] [i]world[/i]");
        if result != "hello world" {
            return Err(format!("Got: '{}'", result));
        }
        Ok(())
    });

    g.run_test("bbcode_strip", || {
        let result = bbcode::strip_bbcode("[color=red][b]test[/b][/color]");
        if result != "test" {
            return Err(format!("Got: '{}'", result));
        }
        Ok(())
    });

    g.run_test("bbcode_to_ansi", || {
        let result = BBCodeParser::to_ansi("[b]bold[/b]");
        if !result.contains("\x1b[1m") {
            return Err(format!("Missing ANSI bold code: {:?}", result));
        }
        Ok(())
    });

    g
}

// ── 3. MessageBuilder ────────────────────────────────────────────────

pub fn test_message_builder() -> TestGroup {
    let mut g = TestGroup::new("MessageBuilder");

    g.run_test("msgbuilder_bold", || {
        let msg = MessageBuilder::new().bold("hi").build();
        if msg != "[b]hi[/b]" {
            return Err(format!("Got: '{}'", msg));
        }
        Ok(())
    });

    g.run_test("msgbuilder_italic", || {
        let msg = MessageBuilder::new().italic("hi").build();
        if msg != "[i]hi[/i]" {
            return Err(format!("Got: '{}'", msg));
        }
        Ok(())
    });

    g.run_test("msgbuilder_underline", || {
        let msg = MessageBuilder::new().underline("hi").build();
        if msg != "[u]hi[/u]" {
            return Err(format!("Got: '{}'", msg));
        }
        Ok(())
    });

    g.run_test("msgbuilder_color", || {
        let msg = MessageBuilder::new().color("red", "hi").build();
        if msg != "[color=red]hi[/color]" {
            return Err(format!("Got: '{}'", msg));
        }
        Ok(())
    });

    g.run_test("msgbuilder_url", || {
        let msg = MessageBuilder::new()
            .url("http://x.com", Some("Link"))
            .build();
        if msg != "[url=http://x.com]Link[/url]" {
            return Err(format!("Got: '{}'", msg));
        }
        Ok(())
    });

    g.run_test("msgbuilder_newline", || {
        let msg = MessageBuilder::new().text("a").newline().text("b").build();
        if msg != "a\nb" {
            return Err(format!("Got: '{}'", msg));
        }
        Ok(())
    });

    g.run_test("msgbuilder_chained", || {
        let msg = MessageBuilder::new()
            .bold("hello")
            .text(" ")
            .italic("world")
            .build();
        if msg != "[b]hello[/b] [i]world[/i]" {
            return Err(format!("Got: '{}'", msg));
        }
        Ok(())
    });

    g
}

// ── 4. ChatHistory ───────────────────────────────────────────────────

pub fn test_chat_history() -> TestGroup {
    let mut g = TestGroup::new("ChatHistory");

    g.run_test("history_add_count", || {
        let mut h = ChatHistory::new(100);
        h.add(ChatMessage::new(1, "Alice", MessageTarget::Server, "msg1"));
        h.add(ChatMessage::new(2, "Bob", MessageTarget::Server, "msg2"));
        h.add(ChatMessage::new(3, "Charlie", MessageTarget::Server, "msg3"));
        if h.total_count() != 3 {
            return Err(format!("Expected 3, got {}", h.total_count()));
        }
        Ok(())
    });

    g.run_test("history_server_messages", || {
        let mut h = ChatHistory::new(100);
        h.add(ChatMessage::new(1, "Alice", MessageTarget::Server, "hello server"));
        let msgs: Vec<_> = h.server_messages().collect();
        if msgs.is_empty() {
            return Err("No server messages found".into());
        }
        if msgs[0].content != "hello server" {
            return Err(format!("Wrong content: {}", msgs[0].content));
        }
        Ok(())
    });

    g.run_test("history_channel_messages", || {
        let mut h = ChatHistory::new(100);
        h.add(ChatMessage::new(1, "Alice", MessageTarget::Channel(42), "in channel 42"));
        let msgs42: Vec<_> = h.channel_messages(42).collect();
        let msgs99: Vec<_> = h.channel_messages(99).collect();
        if msgs42.len() != 1 {
            return Err(format!("Expected 1 msg in ch42, got {}", msgs42.len()));
        }
        if !msgs99.is_empty() {
            return Err(format!("Expected 0 msgs in ch99, got {}", msgs99.len()));
        }
        Ok(())
    });

    g.run_test("history_private_messages", || {
        let mut h = ChatHistory::new(100);
        h.add(ChatMessage::new(1, "Alice", MessageTarget::Private(7), "pm for 7"));
        let msgs: Vec<_> = h.private_messages(7).collect();
        if msgs.len() != 1 {
            return Err(format!("Expected 1, got {}", msgs.len()));
        }
        Ok(())
    });

    g.run_test("history_search", || {
        let mut h = ChatHistory::new(100);
        h.add(ChatMessage::new(1, "Alice", MessageTarget::Server, "hello world"));
        h.add(ChatMessage::new(2, "Bob", MessageTarget::Server, "goodbye world"));
        h.add(ChatMessage::new(3, "Charlie", MessageTarget::Server, "hello again"));
        let results = h.search("hello");
        if results.len() != 2 {
            return Err(format!("Expected 2 results, got {}", results.len()));
        }
        Ok(())
    });

    g.run_test("history_from_sender", || {
        let mut h = ChatHistory::new(100);
        h.add(ChatMessage::new(1, "Alice", MessageTarget::Server, "msg from 1"));
        h.add(ChatMessage::new(2, "Bob", MessageTarget::Server, "msg from 2"));
        h.add(ChatMessage::new(1, "Alice", MessageTarget::Server, "another from 1"));
        let results = h.from_sender(1);
        if results.len() != 2 {
            return Err(format!("Expected 2, got {}", results.len()));
        }
        Ok(())
    });

    g.run_test("history_clear", || {
        let mut h = ChatHistory::new(100);
        h.add(ChatMessage::new(1, "Alice", MessageTarget::Server, "msg"));
        h.clear();
        if h.total_count() != 0 {
            return Err(format!("Expected 0 after clear, got {}", h.total_count()));
        }
        Ok(())
    });

    g.run_test("history_max_capacity", || {
        let mut h = ChatHistory::new(2);
        h.add(ChatMessage::new(1, "A", MessageTarget::Server, "first"));
        h.add(ChatMessage::new(2, "B", MessageTarget::Server, "second"));
        h.add(ChatMessage::new(3, "C", MessageTarget::Server, "third"));
        if h.total_count() != 2 {
            return Err(format!("Expected 2 (capacity limit), got {}", h.total_count()));
        }
        let msgs: Vec<_> = h.server_messages().collect();
        if msgs[0].content != "second" {
            return Err(format!("Expected oldest='second', got '{}'", msgs[0].content));
        }
        Ok(())
    });

    g
}

// ── 5. ChannelTree ───────────────────────────────────────────────────

pub fn test_channel_tree() -> TestGroup {
    let mut g = TestGroup::new("ChannelTree");

    g.run_test("tree_from_channels", || {
        let channels = vec![
            make_channel(1, 0, "Default", 0),
            make_channel(2, 0, "Lobby", 1),
            make_channel(3, 1, "Sub1", 0),
            make_channel(4, 1, "Sub2", 1),
            make_channel(5, 3, "SubSub1", 0),
        ];
        let tree = ChannelTree::from_channels(channels);
        if tree.len() != 5 {
            return Err(format!("Expected 5, got {}", tree.len()));
        }
        Ok(())
    });

    g.run_test("tree_roots", || {
        let channels = vec![
            make_channel(1, 0, "Root1", 0),
            make_channel(2, 0, "Root2", 1),
            make_channel(3, 1, "Child", 0),
        ];
        let tree = ChannelTree::from_channels(channels);
        let roots = tree.roots();
        if roots.len() != 2 {
            return Err(format!("Expected 2 roots, got {}", roots.len()));
        }
        Ok(())
    });

    g.run_test("tree_children", || {
        let channels = vec![
            make_channel(1, 0, "Root", 0),
            make_channel(2, 1, "Child1", 0),
            make_channel(3, 1, "Child2", 1),
        ];
        let tree = ChannelTree::from_channels(channels);
        let children = tree.children(1);
        if children.len() != 2 {
            return Err(format!("Expected 2 children, got {}", children.len()));
        }
        Ok(())
    });

    g.run_test("tree_add_remove", || {
        let mut tree = ChannelTree::new();
        tree.add_channel(make_channel(10, 0, "Test", 0));
        if tree.get(10).is_none() {
            return Err("Channel 10 not found after add".into());
        }
        tree.remove_channel(10);
        if tree.get(10).is_some() {
            return Err("Channel 10 still present after remove".into());
        }
        Ok(())
    });

    g.run_test("tree_path_to", || {
        let channels = vec![
            make_channel(1, 0, "Root", 0),
            make_channel(2, 1, "Mid", 0),
            make_channel(3, 2, "Leaf", 0),
        ];
        let tree = ChannelTree::from_channels(channels);
        let path = tree.path_to(3);
        let names: Vec<&str> = path.iter().map(|c| c.name.as_str()).collect();
        if names != vec!["Root", "Mid", "Leaf"] {
            return Err(format!("Expected [Root, Mid, Leaf], got {:?}", names));
        }
        Ok(())
    });

    g.run_test("tree_find_by_name", || {
        let channels = vec![
            make_channel(1, 0, "Lobby", 0),
            make_channel(2, 0, "Music", 1),
        ];
        let tree = ChannelTree::from_channels(channels);
        match tree.find_by_name("Music") {
            Some(ch) if ch.id == 2 => Ok(()),
            Some(ch) => Err(format!("Wrong channel id: {}", ch.id)),
            None => Err("Channel 'Music' not found".into()),
        }
    });

    g.run_test("tree_print_tree", || {
        let channels = vec![
            make_channel(1, 0, "Root", 0),
            make_channel(2, 1, "Child", 0),
        ];
        let tree = ChannelTree::from_channels(channels);
        let output = tree.print_tree();
        if output.is_empty() {
            return Err("print_tree returned empty string".into());
        }
        if !output.contains("Root") || !output.contains("Child") {
            return Err(format!("Missing names in output: {}", output));
        }
        Ok(())
    });

    g
}

// ── 6. Opus Codec ────────────────────────────────────────────────────

pub fn test_opus_codec() -> TestGroup {
    let mut g = TestGroup::new("Opus Codec");

    g.run_test("codec_create", || {
        OpusCodec::new(AudioConfig::default()).map_err(|e| e.to_string())?;
        Ok(())
    });

    g.run_test("codec_encoder_decoder", || {
        let codec = OpusCodec::new(AudioConfig::default()).map_err(|e| e.to_string())?;
        codec.create_encoder().map_err(|e| e.to_string())?;
        codec.create_decoder().map_err(|e| e.to_string())?;
        Ok(())
    });

    g.run_test("codec_encode_decode_roundtrip", || {
        let config = AudioConfig::default();
        let frame_size = config.frame_size_samples();
        let codec = OpusCodec::new(config).map_err(|e| e.to_string())?;
        let mut encoder = codec.create_encoder().map_err(|e| e.to_string())?;
        let mut decoder = codec.create_decoder().map_err(|e| e.to_string())?;

        // Generate 440Hz sine wave
        let pcm: Vec<i16> = (0..frame_size)
            .map(|i| {
                let t = i as f32 / 48000.0;
                (f32::sin(2.0 * std::f32::consts::PI * 440.0 * t) * 16000.0) as i16
            })
            .collect();

        let mut encoded = vec![0u8; 1024];
        let enc_len = encoder.encode(&pcm, &mut encoded).map_err(|e| e.to_string())?;
        if enc_len == 0 {
            return Err("Encoded length is 0".into());
        }

        let mut decoded = vec![0i16; frame_size];
        let dec_len = decoder
            .decode(&encoded[..enc_len], &mut decoded)
            .map_err(|e| e.to_string())?;
        if dec_len == 0 {
            return Err("Decoded length is 0".into());
        }

        // Check energy is non-zero
        let energy: f64 = decoded.iter().map(|&s| (s as f64).powi(2)).sum();
        if energy < 1.0 {
            return Err(format!("Decoded energy too low: {}", energy));
        }
        Ok(())
    });

    g.run_test("codec_decode_plc", || {
        let config = AudioConfig::default();
        let frame_size = config.frame_size_samples();
        let codec = OpusCodec::new(config).map_err(|e| e.to_string())?;
        let mut decoder = codec.create_decoder().map_err(|e| e.to_string())?;
        let mut output = vec![0i16; frame_size];
        // PLC should not error
        decoder.decode_plc(&mut output).map_err(|e| e.to_string())?;
        Ok(())
    });

    g
}

// ── 7. AudioConfig ───────────────────────────────────────────────────

pub fn test_audio_config() -> TestGroup {
    let mut g = TestGroup::new("AudioConfig");

    g.run_test("config_default", || {
        let c = AudioConfig::default();
        if c.sample_rate != 48000 {
            return Err(format!("sample_rate: {}", c.sample_rate));
        }
        if c.channels != 1 {
            return Err(format!("channels: {}", c.channels));
        }
        if c.frame_size_ms != 20 {
            return Err(format!("frame_size_ms: {}", c.frame_size_ms));
        }
        Ok(())
    });

    g.run_test("config_music", || {
        let c = AudioConfig::music();
        if c.channels != 2 {
            return Err(format!("channels: {}", c.channels));
        }
        if c.bitrate != 96000 {
            return Err(format!("bitrate: {}", c.bitrate));
        }
        if c.vad_enabled {
            return Err("vad should be disabled for music".into());
        }
        Ok(())
    });

    g.run_test("config_low_latency", || {
        let c = AudioConfig::low_latency();
        if c.frame_size_ms != 10 {
            return Err(format!("frame_size_ms: {}", c.frame_size_ms));
        }
        if c.playback_buffer_ms != 40 {
            return Err(format!("playback_buffer_ms: {}", c.playback_buffer_ms));
        }
        Ok(())
    });

    g.run_test("config_frame_size_samples", || {
        let c = AudioConfig::default();
        let expected = 960; // 48000 * 20 / 1000
        if c.frame_size_samples() != expected {
            return Err(format!("Expected {}, got {}", expected, c.frame_size_samples()));
        }
        Ok(())
    });

    g
}

// ── 8. BotConfig & Commands ──────────────────────────────────────────

pub fn test_bot_config_commands() -> TestGroup {
    let mut g = TestGroup::new("BotConfig & Commands");

    g.run_test("bot_config_builder", || {
        let config = BotConfig::builder()
            .address("localhost")
            .nickname("TestBot")
            .build()
            .map_err(|e| e.to_string())?;
        if config.command_prefix != "!" {
            return Err(format!("prefix: '{}'", config.command_prefix));
        }
        if config.nickname != "TestBot" {
            return Err(format!("nickname: '{}'", config.nickname));
        }
        Ok(())
    });

    g.run_test("bot_config_missing_address", || {
        let result = BotConfig::builder().nickname("Bot").build();
        if result.is_ok() {
            return Err("Expected error for missing address".into());
        }
        Ok(())
    });

    g.run_test("command_registry_parse", || {
        let reg = CommandRegistry::new("!");
        match reg.parse("!ping") {
            Some(("ping", args)) if args.is_empty() => {}
            other => return Err(format!("!ping parse failed: {:?}", other)),
        }
        match reg.parse("!echo a b") {
            Some(("echo", args)) if args == vec!["a", "b"] => {}
            other => return Err(format!("!echo a b parse failed: {:?}", other)),
        }
        if reg.parse("hello").is_some() {
            return Err("'hello' should not parse as command".into());
        }
        Ok(())
    });

    g.run_test("command_registry_register_get", || {
        let mut reg = CommandRegistry::new("!");
        let cmd = Command::new("ping", FnHandler::new(|_ctx| async { Ok(()) }))
            .alias("p")
            .help("Pong!");
        reg.register(cmd);
        if reg.get("ping").is_none() {
            return Err("Command 'ping' not found".into());
        }
        if reg.get("p").is_none() {
            return Err("Alias 'p' not found".into());
        }
        Ok(())
    });

    g
}
