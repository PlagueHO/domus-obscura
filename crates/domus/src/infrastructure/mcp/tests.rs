use super::*;
use crate::application::services::{EntityRef, FetchedArtwork};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{
        "protocolVersion":VERSION,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}
    }})
}

fn dispatch(session: &mut Session, message: Value) -> Action {
    session.dispatch(message.to_string().as_bytes())
}

fn ready() -> Session {
    let mut session = Session::default();
    assert!(matches!(
        dispatch(&mut session, initialize()),
        Action::Reply(_)
    ));
    assert!(matches!(
        dispatch(
            &mut session,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        ),
        Action::Notification(None)
    ));
    session
}

fn reply(action: Action) -> Result<Value> {
    match action {
        Action::Reply(reply) => Ok(reply),
        _ => Err(Error::Internal("expected MCP reply")),
    }
}

fn call(id: Value, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}})
}

fn release() -> Value {
    json!({"provider":"musicbrainz","id":"fixture-release"})
}

fn modern_request(mut message: Value) -> Value {
    message["params"]["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion":MODERN_VERSION,
        "io.modelcontextprotocol/clientCapabilities":{}
    });
    message
}

struct Fixture {
    calls: AtomicUsize,
    completions: AtomicUsize,
    oversized: bool,
    gate: Option<Arc<tokio::sync::Notify>>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            completions: AtomicUsize::new(0),
            oversized: false,
            gate: None,
        }
    }
}

impl ReadServices for Fixture {
    fn call(&self, request: ServiceRequest) -> BoxFuture<'_, Result<ServiceResponse>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            request.validate()?;
            if let Some(gate) = &self.gate {
                gate.notified().await;
            }
            self.completions.fetch_add(1, Ordering::SeqCst);
            Ok(ServiceResponse::ArtworkFetched {
                provider: "cover-art-archive".into(),
                release: EntityRef {
                    provider: "musicbrainz".into(),
                    id: "fixture-release".into(),
                },
                sources: if self.oversized {
                    vec!["x".repeat(MAX_RESULT)]
                } else {
                    vec![]
                },
                image: Some(FetchedArtwork {
                    sha256: "a".repeat(64),
                    bytes: 123,
                }),
            })
        })
    }
}

fn exchange(host: &Fixture, messages: Vec<Value>) -> Result<Vec<Value>> {
    let mut input = messages
        .into_iter()
        .map(|message| format!("{message}\n"))
        .collect::<String>();
    input.shrink_to_fit();
    let (sender, mut receiver) = mpsc::channel(2);
    let reader =
        std::thread::spawn(move || read_messages(&mut std::io::Cursor::new(input), sender));
    let mut output = Vec::new();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(serve(host, &mut receiver, &mut output))?;
    reader
        .join()
        .map_err(|_| Error::Internal("MCP fixture reader panicked"))?;
    output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).map_err(Error::from))
        .collect()
}

#[test]
fn handshake_is_explicit_and_advertises_only_the_implemented_revision() -> Result<()> {
    let mut session = Session::default();
    let premature = reply(dispatch(
        &mut session,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    ))?;
    assert_eq!(premature["error"]["code"], -32602);
    let response = reply(dispatch(&mut session, initialize()))?;
    assert_eq!(response["id"], "init");
    assert_eq!(response["result"]["protocolVersion"], VERSION);
    assert!(response["result"]["capabilities"].get("tools").is_some());
    assert!(response["result"]["capabilities"].get("tasks").is_none());
    assert!(response["result"].get("resultType").is_none());
    assert_eq!(
        reply(dispatch(&mut session, initialize()))?["error"]["code"],
        -32602
    );
    assert_eq!(
        reply(dispatch(
            &mut session,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})
        ))?["error"]["code"],
        -32600
    );
    let mut session = ready();
    assert!(
        reply(dispatch(
            &mut session,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/list"})
        ))?["result"]["tools"]
            .is_array()
    );
    assert_eq!(
        reply(dispatch(
            &mut session,
            json!({"jsonrpc":"2.0","id":4,"method":"server/discover"})
        ))?["error"]["code"],
        -32602
    );
    Ok(())
}

#[test]
fn modern_discovery_and_tools_need_no_handshake_or_previous_client_state() -> Result<()> {
    let mut session = Session::default();
    for method in ["server/discover", "tools/list", "ping"] {
        let response = reply(dispatch(
            &mut session,
            modern_request(json!({"jsonrpc":"2.0","id":method,"method":method})),
        ))?;
        assert_eq!(response["id"], method);
        assert_eq!(response["result"]["resultType"], "complete");
        assert_eq!(
            response["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "domus-obscura"
        );
        assert!(!session.initialized);
        assert!(!session.ready);
        if method == "server/discover" {
            assert_eq!(
                response["result"]["supportedVersions"],
                json!([MODERN_VERSION])
            );
            assert_eq!(response["result"]["capabilities"], json!({"tools":{}}));
        }
        if method == "tools/list" {
            assert_eq!(
                response["result"]["tools"]
                    .as_array()
                    .ok_or(Error::Internal("modern tools"))?
                    .len(),
                4
            );
        }
    }
    Ok(())
}

#[test]
fn modern_metadata_is_checked_for_each_request_and_unknown_versions_are_visible() -> Result<()> {
    let mut session = Session::default();
    let valid = modern_request(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
    assert!(reply(dispatch(&mut session, valid.clone()))?["result"].is_object());
    for key in [PROTOCOL_VERSION, CLIENT_CAPABILITIES] {
        let mut missing = valid.clone();
        missing["params"]["_meta"]
            .as_object_mut()
            .ok_or(Error::Internal("metadata"))?
            .remove(key);
        assert_eq!(
            reply(dispatch(&mut session, missing))?["error"]["code"],
            -32602
        );
    }
    for (field, value) in [
        (CLIENT_CAPABILITIES, json!([])),
        (PROTOCOL_VERSION, json!(7)),
        (CLIENT_INFO, Value::Null),
        (CLIENT_INFO, json!({"name":"fixture"})),
    ] {
        let mut invalid = valid.clone();
        invalid["params"]["_meta"][field] = value;
        assert_eq!(
            reply(dispatch(&mut session, invalid))?["error"]["code"],
            -32602
        );
    }
    let mut unknown = valid.clone();
    unknown["params"]["_meta"][PROTOCOL_VERSION] = json!("1900-01-01");
    let response = reply(dispatch(&mut session, unknown))?;
    assert_eq!(response["error"]["code"], -32022);
    assert_eq!(
        response["error"]["data"],
        json!({"supported":[MODERN_VERSION],"requested":"1900-01-01"})
    );
    let mut identity = valid;
    identity["params"]["_meta"][CLIENT_INFO] = json!({"name":"unverified-client","version":"1"});
    let response = reply(dispatch(&mut session, identity))?;
    assert_eq!(response["result"]["resultType"], "complete");
    assert!(!response.to_string().contains("unverified-client"));
    Ok(())
}

#[test]
fn modern_read_results_keep_the_shared_digest_only_contract() -> Result<()> {
    let host = Fixture::new();
    let replies = exchange(
        &host,
        vec![modern_request(call(
            json!("read"),
            "artwork_fetch",
            json!({"provider":"cover-art-archive","release":release(),"image_id":"image-1"}),
        ))],
    )?;
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.completions.load(Ordering::SeqCst), 1);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["id"], "read");
    let result = &replies[0]["result"];
    assert_eq!(result["resultType"], "complete");
    assert_eq!(result["isError"], false);
    assert_eq!(
        result["structuredContent"]["image"],
        json!({"sha256":"a".repeat(64),"bytes":123})
    );
    Ok(())
}

#[test]
fn modern_requests_do_not_borrow_legacy_initialization_or_capabilities() -> Result<()> {
    let mut session = ready();
    let mut invalid = modern_request(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
    invalid["params"]["_meta"]
        .as_object_mut()
        .ok_or(Error::Internal("metadata"))?
        .remove(CLIENT_CAPABILITIES);
    assert_eq!(
        reply(dispatch(&mut session, invalid))?["error"]["code"],
        -32602
    );
    assert_eq!(
        reply(dispatch(
            &mut session,
            modern_request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        ))?["result"]["resultType"],
        "complete"
    );
    assert!(
        reply(dispatch(
            &mut session,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/list"})
        ))?["result"]
            .get("resultType")
            .is_none()
    );
    assert_eq!(
        reply(dispatch(&mut session, modern_request(initialize())))?["error"]["code"],
        -32601
    );
    assert!(session.initialized && session.ready);
    let mut fresh = Session::default();
    reply(dispatch(
        &mut fresh,
        modern_request(json!({"jsonrpc":"2.0","id":4,"method":"server/discover"})),
    ))?;
    assert_eq!(
        reply(dispatch(&mut fresh, initialize()))?["result"]["protocolVersion"],
        VERSION
    );
    assert!(!fresh.ready);
    assert_eq!(
        reply(dispatch(
            &mut fresh,
            modern_request(json!({"jsonrpc":"2.0","id":5,"method":"tools/list"}))
        ))?["result"]["resultType"],
        "complete"
    );
    Ok(())
}

#[test]
fn malformed_modern_ids_are_omitted_and_legacy_unknown_ids_remain_null() -> Result<()> {
    for mut session in [Session::default(), ready()] {
        let response = reply(dispatch(
            &mut session,
            modern_request(json!({"jsonrpc":"2.0","id":null,"method":"ping"})),
        ))?;
        assert_eq!(response["error"]["code"], -32600);
        assert!(response.get("id").is_none());
    }
    let mut legacy = ready();
    assert_eq!(reply(legacy.dispatch(b"{"))?.get("id"), Some(&Value::Null));
    let mut modern = Session::default();
    assert!(reply(modern.dispatch(b"{"))?.get("id").is_none());
    Ok(())
}

#[test]
fn modern_invalid_arguments_are_complete_tool_errors_not_mutations() -> Result<()> {
    let mut session = Session::default();
    let response = reply(dispatch(
        &mut session,
        modern_request(call(
            json!("bad"),
            "release_metadata",
            json!({"release":release(),"credentials":"never-echo"}),
        )),
    ))?;
    assert_eq!(response["result"]["resultType"], "complete");
    assert_eq!(response["result"]["isError"], true);
    assert!(!response.to_string().contains("never-echo"));
    assert_eq!(
        reply(dispatch(
            &mut session,
            modern_request(call(json!(2), "acquire", json!({})))
        ))?["error"]["code"],
        -32602
    );
    let mut discover = modern_request(json!({"jsonrpc":"2.0","id":3,"method":"server/discover"}));
    discover["params"]["database"] = json!("never-echo");
    assert_eq!(
        reply(dispatch(&mut session, discover))?["error"]["code"],
        -32602
    );
    Ok(())
}

#[test]
fn notifications_never_start_services_or_receive_replies() -> Result<()> {
    let host = Fixture::new();
    let mut notification = call(json!(1), "release_metadata", json!({"release":release()}));
    notification
        .as_object_mut()
        .ok_or(Error::Internal("fixture object"))?
        .remove("id");
    let responses = exchange(
        &host,
        vec![
            initialize(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            notification,
            json!({"jsonrpc":"2.0","method":"unknown","params":[]}),
            json!({"jsonrpc":"2.0","method":"unknown","unexpected":true}),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"missing"}}),
        ],
    )?;
    assert_eq!(responses.len(), 1);
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn malformed_frames_ids_batches_and_duplicate_fields_are_not_commands() -> Result<()> {
    let mut session = ready();
    assert_eq!(reply(session.dispatch(b"{"))?["error"]["code"], -32700);
    assert_eq!(reply(session.dispatch(b"\xff\n"))?["error"]["code"], -32700);
    for message in [
        json!([]),
        json!({"jsonrpc":"2.0","id":null,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":1.5,"method":"ping"}),
        json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":true,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":1,"method":"ping","params":[]}),
        json!({"jsonrpc":"2.0","id":1,"method":"ping","params":null}),
    ] {
        assert_eq!(
            reply(dispatch(&mut session, message))?["error"]["code"],
            -32600
        );
    }

    assert_eq!(
        reply(session.dispatch(br#"{"jsonrpc":"2.0","id":1,"id":2,"method":"ping"}"#))?["error"]["code"],
        -32600
    );
    for id in [json!("browser-42"), json!(-3), json!(0), json!(u64::MAX)] {
        assert_eq!(
            reply(dispatch(
                &mut session,
                json!({"jsonrpc":"2.0","id":id,"method":"ping"})
            ))?["id"],
            id
        );
    }
    Ok(())
}

#[tokio::test]
async fn transport_failures_drain_the_current_provider_before_returning_error() -> Result<()> {
    struct Output {
        fail: bool,
        messages: usize,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.fail && self.messages > 0 {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            self.messages += bytes.iter().filter(|byte| **byte == b'\n').count();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    for is_modern in [false, true] {
        for failed_output in [false, true] {
            let gate = Arc::new(tokio::sync::Notify::new());
            let mut host = Fixture::new();
            host.gate = Some(gate.clone());
            let (sender, mut receiver) = mpsc::channel(2);
            let producer = tokio::spawn(async move {
                for mut message in [
                    initialize(),
                    json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                    call(
                        json!("active"),
                        "release_metadata",
                        json!({"release":release()}),
                    ),
                ] {
                    if is_modern && message["method"] == "tools/call" {
                        message = modern_request(message);
                    }
                    sender
                        .send(Ok(message.to_string().into_bytes()))
                        .await
                        .map_err(|_| Error::Internal("fixture input closed"))?;
                }
                let message = if failed_output {
                    Ok(json!({"jsonrpc":"2.0","id":"ping","method":"ping"})
                        .to_string()
                        .into_bytes())
                } else {
                    Err(Error::InvalidInput("fixture framing failure".into()))
                };
                sender
                    .send(message)
                    .await
                    .map_err(|_| Error::Internal("fixture failure not delivered"))?;
                tokio::time::timeout(std::time::Duration::from_secs(5), sender.closed())
                    .await
                    .map_err(|_| Error::Internal("server did not close failed transport"))?;
                gate.notify_one();
                Ok::<(), Error>(())
            });
            let mut output = Output {
                fail: failed_output,
                messages: 0,
            };
            let outcome = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                serve(&host, &mut receiver, &mut output),
            )
            .await
            .map_err(|_| Error::Internal("transport drain fixture timed out"))?;
            producer
                .await
                .map_err(|_| Error::Internal("fixture producer panicked"))??;
            assert!(outcome.is_err());
            assert_eq!(host.calls.load(Ordering::SeqCst), 1);
            assert_eq!(host.completions.load(Ordering::SeqCst), 1);
            assert_eq!(output.messages, 1);
        }
    }
    Ok(())
}

#[test]
fn only_the_four_shared_read_operations_can_be_dispatched() -> Result<()> {
    let mut session = ready();
    let advertised = reply(dispatch(
        &mut session,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    ))?;
    let tools = advertised["result"]["tools"]
        .as_array()
        .ok_or(Error::Internal("tool list"))?;
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>(),
        vec![
            Some("catalog_discover"),
            Some("release_metadata"),
            Some("artwork_list"),
            Some("artwork_fetch")
        ]
    );
    for (name, arguments, operation) in [
        (
            "catalog_discover",
            json!({"request":{"query":{"operation":"artists","text":"fixture","offset":0,"limit":1}}}),
            "discover",
        ),
        ("release_metadata", json!({"release":release()}), "metadata"),
        (
            "artwork_list",
            json!({"provider":"cover-art-archive","release":release()}),
            "artwork_list",
        ),
        (
            "artwork_fetch",
            json!({"provider":"cover-art-archive","release":release(),"image_id":"image-1"}),
            "artwork_fetch",
        ),
    ] {
        let Action::Call { request, .. } = dispatch(&mut session, call(json!(2), name, arguments))
        else {
            return Err(Error::Internal("valid read request was rejected"));
        };
        assert_eq!(serde_json::to_value(request)?["operation"], operation);
    }
    for name in [
        "approve",
        "acquire",
        "publish",
        "job_control",
        "service-call",
        "filesystem_write",
    ] {
        assert_eq!(
            reply(dispatch(&mut session, call(json!(3), name, json!({}))))?["error"]["code"],
            -32602
        );
    }
    Ok(())
}

#[test]
fn tool_arguments_reject_private_state_mutations_and_unknown_nested_fields() -> Result<()> {
    let mut session = ready();
    for arguments in [
        json!({"release":release(),"database":"private.db"}),
        json!({"release":release(),"credentials":{"password":"never echo this"}}),
        json!({"release":release(),"operation":"acquire"}),
        json!({"release":{"provider":"musicbrainz","id":"fixture","path":"outside"}}),
        json!({"release":{"provider":"musicbrainz","id":""}}),
    ] {
        let response = reply(dispatch(
            &mut session,
            call(json!(2), "release_metadata", arguments),
        ))?;
        assert_eq!(response["result"]["isError"], true);
        assert!(!response.to_string().contains("never echo this"));
        assert!(!response.to_string().contains("private.db"));
    }
    for query in [
        json!({"operation":"artists","text":"fixture","offset":0,"limit":26}),
        json!({"operation":"artists","text":"fixture","offset":0,"limit":1,"destination":"outside"}),
        json!({"operation":"albums","text":"fixture","offset":0,"limit":1,"filters":{"include_singles":false,"include_live":false,"include_compilations":false,"include_unofficial":false,"credentials":"forbidden"}}),
    ] {
        assert_eq!(
            reply(dispatch(
                &mut session,
                call(
                    json!(2),
                    "catalog_discover",
                    json!({"request":{"query":query}})
                )
            ))?["result"]["isError"],
            true
        );
    }
    assert_eq!(
        reply(dispatch(
            &mut session,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/list","params":{"cursor":"unknown"}})
        ))?["error"]["code"],
        -32602
    );
    Ok(())
}

#[test]
fn completed_read_is_structured_digest_only_and_stdout_is_json_lines() -> Result<()> {
    let host = Fixture::new();
    let replies = exchange(
        &host,
        vec![
            initialize(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            call(
                json!("fetch"),
                "artwork_fetch",
                json!({"provider":"cover-art-archive","release":release(),"image_id":"image-1"}),
            ),
        ],
    )?;
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    let result = &replies
        .last()
        .ok_or(Error::Internal("missing call response"))?["result"];
    assert_eq!(result["isError"], false);
    assert_eq!(
        result["structuredContent"]["image"],
        json!({"sha256":"a".repeat(64),"bytes":123})
    );
    let text: Value = serde_json::from_str(
        result["content"][0]["text"]
            .as_str()
            .ok_or(Error::Internal("text result"))?,
    )?;
    assert_eq!(text, result["structuredContent"]);
    assert!(result["structuredContent"].get("database").is_none());
    Ok(())
}

#[test]
fn oversized_results_are_explicit_errors_without_partial_content() -> Result<()> {
    let mut host = Fixture::new();
    host.oversized = true;
    let replies = exchange(
        &host,
        vec![
            initialize(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            call(json!(2), "release_metadata", json!({"release":release()})),
        ],
    )?;
    let result = &replies
        .last()
        .ok_or(Error::Internal("missing size response"))?["result"];
    assert_eq!(result["isError"], true);
    assert!(result.get("structuredContent").is_none());
    assert!(result.to_string().len() < 256);
    Ok(())
}

#[test]
fn frame_reader_has_an_exact_bound_and_requires_newlines() -> Result<()> {
    for (bytes, valid) in [
        (vec![], true),
        (b"{}\n".to_vec(), true),
        ([vec![b' '; MAX_INPUT - 1], vec![b'\n']].concat(), true),
        ([vec![b' '; MAX_INPUT], vec![b'\n']].concat(), false),
        (b"{}".to_vec(), false),
    ] {
        let (sender, mut receiver) = mpsc::channel(2);
        let empty = bytes.is_empty();
        read_messages(&mut std::io::Cursor::new(bytes), sender);
        let result = receiver.blocking_recv();
        if empty {
            assert!(result.is_none());
        } else {
            assert_eq!(
                result.ok_or(Error::Internal("missing frame"))?.is_ok(),
                valid
            );
        }
        assert!(receiver.blocking_recv().is_none());
    }
    Ok(())
}

#[test]
fn broken_output_is_reported_instead_of_success() -> Result<()> {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert!(write_message(&mut Broken, &json!({"jsonrpc":"2.0","id":1,"result":{}})).is_err());
    assert!(encode(&"x".repeat(20), 10)?.is_none());
    Ok(())
}

#[test]
fn wire_output_has_an_exact_four_mib_bound_including_its_newline() -> Result<()> {
    let mut message = json!({"jsonrpc":"2.0","id":1,"result":{"data":""}});
    let overhead = serde_json::to_vec(&message)?.len() + 1;
    let mut text = format!("\u{e9}{}", "x".repeat(MAX_OUTPUT - overhead - 2));
    message["result"]["data"] = json!(text);
    let mut output = vec![];
    write_message(&mut output, &message)?;
    assert_eq!(output.len(), MAX_OUTPUT);
    assert_eq!(output.last(), Some(&b'\n'));
    text.push('x');
    message["result"]["data"] = json!(text);
    output.clear();
    assert!(write_message(&mut output, &message).is_err());
    assert!(output.is_empty());
    Ok(())
}

#[test]
fn service_result_bound_counts_exact_serialized_json_bytes() -> Result<()> {
    let make = |source: String| ServiceResponse::ArtworkFetched {
        provider: "cover-art-archive".into(),
        release: EntityRef {
            provider: "musicbrainz".into(),
            id: "fixture".into(),
        },
        sources: vec![source],
        image: None,
    };
    let overhead = serde_json::to_vec(&make(String::new()))?.len();
    let text = format!("\u{e9}{}", "x".repeat(MAX_RESULT - overhead - 2));
    let result = tool_result(Ok(make(text.clone())))?;
    assert_eq!(result["isError"], false);
    assert_eq!(
        result["content"][0]["text"]
            .as_str()
            .ok_or(Error::Internal("bounded JSON result"))?
            .len(),
        MAX_RESULT
    );
    let result = tool_result(Ok(make(format!("{text}x"))))?;
    assert_eq!(result["isError"], true);
    assert!(result.get("structuredContent").is_none());
    Ok(())
}

#[tokio::test]
async fn cancellation_keeps_provider_completion_but_suppresses_its_reply() -> Result<()> {
    for is_modern in [false, true] {
        cancellation_case(is_modern).await?;
    }
    Ok(())
}

async fn cancellation_case(is_modern: bool) -> Result<()> {
    struct ObservedWriter {
        bytes: Vec<u8>,
        ping: Arc<tokio::sync::Notify>,
    }
    impl Write for ObservedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            if bytes.contains(&b'\n') && self.bytes.windows(9).any(|slice| slice == b"\"id\":99,\"")
            {
                self.ping.notify_one();
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let gate = Arc::new(tokio::sync::Notify::new());
    let ping = Arc::new(tokio::sync::Notify::new());
    let mut host = Fixture::new();
    host.gate = Some(gate.clone());
    let (sender, mut receiver) = mpsc::channel(2);
    let producer = tokio::spawn({
        let ping = ping.clone();
        async move {
            let messages = [
                initialize(),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                call(
                    json!("active"),
                    "release_metadata",
                    json!({"release":release()}),
                ),
                call(
                    json!("busy"),
                    "release_metadata",
                    json!({"release":release()}),
                ),
                json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"active"}}),
                json!({"jsonrpc":"2.0","id":99,"method":"ping"}),
            ];
            for mut message in messages {
                if is_modern && message.get("id").is_some() && message["method"] != "initialize" {
                    message = modern_request(message);
                }
                sender
                    .send(Ok(message.to_string().into_bytes()))
                    .await
                    .map_err(|_| Error::Internal("fixture input closed"))?;
            }
            tokio::time::timeout(std::time::Duration::from_secs(5), ping.notified())
                .await
                .map_err(|_| Error::Internal("fixture ping was not processed"))?;
            gate.notify_one();
            Ok::<(), Error>(())
        }
    });
    let mut output = ObservedWriter {
        bytes: vec![],
        ping,
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        serve(&host, &mut receiver, &mut output),
    )
    .await
    .map_err(|_| Error::Internal("MCP cancellation fixture timed out"))??;
    producer
        .await
        .map_err(|_| Error::Internal("fixture producer panicked"))??;
    let replies: Vec<Value> = output
        .bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice)
        .collect::<std::result::Result<_, _>>()?;
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.completions.load(Ordering::SeqCst), 1);
    assert!(!replies.iter().any(|reply| reply["id"] == "active"));
    assert!(
        replies
            .iter()
            .any(|reply| reply["id"] == "busy" && reply["error"]["code"] == -31000)
    );
    assert!(replies.iter().any(|reply| reply["id"] == 99));
    Ok(())
}
