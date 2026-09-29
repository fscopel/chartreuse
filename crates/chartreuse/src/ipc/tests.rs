use std::fs;
use std::io::{BufReader, Write as _};
use std::path::Path;

use chartreuse_core::color::Rgba8;
use chartreuse_core::geometry::PhysicalSize;
use chartreuse_core::image::Image;
use chartreuse_imaging::Format;
use chartreuse_platform::event;
use futures::executor::block_on;
use futures::StreamExt as _;
use iced::advanced::subscription::into_recipes;
use iced_runtime::Action;

use super::*;
use crate::windows::WindowKind;

/// An endpoint of its own for one test, and what keeps it alive.
struct TestEndpoint {
    endpoint: Endpoint,
    #[cfg(unix)]
    _directory: tempfile::TempDir,
}

#[cfg(unix)]
fn test_endpoint() -> TestEndpoint {
    let directory = tempfile::tempdir().unwrap();
    TestEndpoint {
        endpoint: Endpoint::in_directory(directory.path().join("instance")),
        _directory: directory,
    }
}

#[cfg(windows)]
fn test_endpoint() -> TestEndpoint {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let name = format!(
        "chartreuse-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    TestEndpoint {
        endpoint: Endpoint::named(&name),
    }
}

fn claim(endpoint: &Endpoint) -> Listener {
    match connect_or_claim(endpoint).unwrap() {
        Role::Instance(listener) => listener,
        Role::Client(_) => panic!("expected to become the instance"),
    }
}

fn client(endpoint: &Endpoint) -> Stream {
    match connect_or_claim(endpoint).unwrap() {
        Role::Client(stream) => stream,
        Role::Instance(_) => panic!("expected to connect to the instance"),
    }
}

/// The next request the instance received, waiting a while for it.
fn next_request(requests: &EventReceiver<Request>) -> Request {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(request) = requests.try_recv() {
            return request;
        }
        assert!(Instant::now() < deadline, "no request arrived");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Sends `command` from another thread, as a second process would.
fn send_in_background(
    endpoint: &Endpoint,
    command: Command,
) -> thread::JoinHandle<Result<(), SendError>> {
    let stream = client(endpoint);
    thread::spawn(move || send(stream, &command))
}

/// Writes `request` as is and returns the reply line.
fn raw_exchange(endpoint: &Endpoint, request: &[u8]) -> Vec<u8> {
    let mut stream = transport::connect(endpoint).unwrap();
    stream.write_all(request).unwrap();
    stream.flush().unwrap();
    protocol::read_line(&mut BufReader::new(stream))
        .unwrap()
        .expect("a reply")
}

fn absolute(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

#[test]
fn the_first_process_is_the_instance_and_the_next_one_its_client() {
    let test = test_endpoint();
    let listener = claim(&test.endpoint);
    let connection = client(&test.endpoint);

    // Once the instance is gone, the next process takes its place.
    drop((connection, listener));
    let _next = claim(&test.endpoint);
}

#[test]
fn a_client_that_left_before_being_accepted_does_not_stop_the_instance() {
    let test = test_endpoint();
    let listener = claim(&test.endpoint);
    // Like `chartreuse` without a command, which only checks that the instance
    // runs, before the instance waits for clients.
    drop(client(&test.endpoint));

    let requests = serve(listener).unwrap();
    let sent = send_in_background(&test.endpoint, Command::Capture(CaptureMode::Display));
    assert!(next_request(&requests).answer(Ok(())));
    sent.join().unwrap().unwrap();
}

#[test]
fn a_command_reaches_the_app_and_its_answer_the_client() {
    let test = test_endpoint();
    let requests = serve(claim(&test.endpoint)).unwrap();

    let sent = send_in_background(&test.endpoint, Command::Capture(CaptureMode::Selection));
    let request = next_request(&requests);
    assert_eq!(request.command(), &Command::Capture(CaptureMode::Selection));
    request.answer(Ok(()));
    sent.join().unwrap().unwrap();

    let path = absolute("with a space\nand a line break.png");
    let sent = send_in_background(&test.endpoint, Command::Open(path.clone()));
    let request = next_request(&requests);
    assert_eq!(request.command(), &Command::Open(path));
    request.answer(Err("a capture is already in progress".into()));
    let refused = sent.join().unwrap().unwrap_err();
    assert!(matches!(&refused, SendError::Refused(_)), "{refused:?}");
    assert_eq!(refused.to_string(), "a capture is already in progress");
}

#[test]
fn malformed_requests_are_refused_without_reaching_the_app() {
    let test = test_endpoint();
    let requests = serve(claim(&test.endpoint)).unwrap();

    for request in [
        &b"capture everything\n"[..],
        b"open relative.png\n",
        b"screenshot\n",
    ] {
        let reply = raw_exchange(&test.endpoint, request);
        let refusal = protocol::decode_reply(&reply).unwrap();
        assert!(refusal.is_err(), "{request:?} got {refusal:?}");
    }
    // A client that leaves without a request is no trouble either.
    drop(transport::connect(&test.endpoint).unwrap());

    let sent = send_in_background(&test.endpoint, Command::Capture(CaptureMode::Selection));
    let request = next_request(&requests);
    assert_eq!(request.command(), &Command::Capture(CaptureMode::Selection));
    request.answer(Ok(()));
    sent.join().unwrap().unwrap();
    assert!(
        requests.try_recv().is_none(),
        "only the valid request arrived"
    );
}

#[test]
fn commands_to_an_app_that_quit_are_refused() {
    let test = test_endpoint();
    let requests = serve(claim(&test.endpoint)).unwrap();
    drop(requests);

    let refused = send_in_background(&test.endpoint, Command::Capture(CaptureMode::Display))
        .join()
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(&refused, SendError::Refused(reason) if reason.ends_with("is quitting")),
        "{refused:?}"
    );
}

#[test]
fn an_unanswered_request_is_refused() {
    let (request, answer) = Request::new(Command::Capture(CaptureMode::Display));
    drop(request);
    assert!(answer.wait(Duration::ZERO).is_err());

    // Once the client stopped waiting, no answer reaches it.
    let (request, answer) = Request::new(Command::Capture(CaptureMode::Display));
    assert!(answer.wait(Duration::from_millis(1)).is_err());
    assert!(!request.answer(Ok(())));
}

#[test]
fn the_first_answer_counts() {
    let (request, answer) = Request::new(Command::Capture(CaptureMode::Display));
    assert!(request.clone().answer(Err("refused".to_owned())));
    assert!(!request.answer(Ok(())));
    assert_eq!(answer.wait(Duration::ZERO), Err("refused".to_owned()));
}

#[cfg(unix)]
#[test]
fn a_leftover_socket_is_replaced() {
    use std::os::unix::net::{UnixListener, UnixStream};

    let test = test_endpoint();
    // An instance that ended: its socket file stays, nobody listens.
    let socket = test.endpoint.socket();
    fs::create_dir_all(socket.parent().unwrap()).unwrap();
    drop(UnixListener::bind(&socket).unwrap());
    assert!(socket.exists());
    assert!(UnixStream::connect(&socket).is_err());

    let requests = serve(claim(&test.endpoint)).unwrap();
    let sent = send_in_background(&test.endpoint, Command::Capture(CaptureMode::Display));
    next_request(&requests).answer(Ok(()));
    sent.join().unwrap().unwrap();
}

fn sample() -> Image {
    Image::from_fn(PhysicalSize::new(3, 2), |x, y| {
        Rgba8::new(
            u8::try_from(x * 80).unwrap(),
            u8::try_from(y * 90).unwrap(),
            40,
            255,
        )
    })
}

fn write_sample(path: &Path) {
    fs::write(
        path,
        chartreuse_imaging::encode(&sample(), Format::Png).unwrap(),
    )
    .unwrap();
}

/// Delivers `command` as a request from another process and returns the
/// answer.
fn receive(app: &mut App, command: Command) -> Result<(), String> {
    let (request, answer) = Request::new(command);
    let _ = app.settle(AppMessage::Ipc(Message::Received(request)));
    answer.wait(Duration::ZERO)
}

fn count(app: &App, kind: WindowKind) -> usize {
    app.windows.of_kind(kind).count()
}

#[test]
fn commands_are_what_the_status_item_sends() {
    for mode in CaptureMode::ALL {
        assert!(matches!(
            Command::Capture(mode).message(),
            AppMessage::Capture(capture::Message::Start(started)) if started == mode
        ));
    }
    let path = absolute("picture.png");
    assert!(matches!(
        Command::Open(path.clone()).message(),
        AppMessage::Import(import::Message::OpenPath(opened)) if opened == path
    ));
}

#[test]
fn a_received_capture_starts_and_a_second_one_is_refused() {
    let (mut app, _fake) = App::for_test();
    assert_eq!(
        receive(&mut app, Command::Capture(CaptureMode::Selection)),
        Ok(())
    );
    assert_eq!(app.capture.in_progress(), Some(CaptureMode::Selection));
    let overlays = count(&app, WindowKind::Overlay);
    assert!(overlays > 0, "the selection overlays opened");

    let refused = receive(&mut app, Command::Capture(CaptureMode::Display));
    assert_eq!(refused, Err("a capture is already in progress".to_owned()));
    assert_eq!(app.capture.in_progress(), Some(CaptureMode::Selection));
    assert_eq!(count(&app, WindowKind::Overlay), overlays);
}

#[test]
fn a_command_whose_client_stopped_waiting_does_not_run() {
    let (mut app, _fake) = App::for_test();
    let (request, answer) = Request::new(Command::Capture(CaptureMode::Selection));
    // The client was told the command was not taken before the app got to it.
    assert!(answer.wait(Duration::ZERO).is_err());

    let _ = app.settle(AppMessage::Ipc(Message::Received(request)));
    assert_eq!(app.capture.in_progress(), None);
    assert_eq!(count(&app, WindowKind::Overlay), 0);
}

#[test]
fn a_received_file_opens_in_an_editor_even_during_a_capture() {
    let (mut app, _fake) = App::for_test();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("picture.png");
    write_sample(&path);

    assert_eq!(
        receive(&mut app, Command::Capture(CaptureMode::Selection)),
        Ok(())
    );
    assert_eq!(receive(&mut app, Command::Open(path)), Ok(()));
    let images: Vec<Image> = app
        .windows
        .of_kind(WindowKind::Editor)
        .map(|window| app.editor.get(window).unwrap().document().base().clone())
        .collect();
    assert_eq!(images, [sample()]);
}

#[test]
fn boot_runs_the_command_lines_command() {
    let (mut app, _fake) = App::for_test();
    assert!(iced_runtime::task::into_stream(boot(&mut app)).is_none());

    app.ipc = State::new(None, Some(Command::Capture(CaptureMode::Selection)));
    let actions: Vec<Action<AppMessage>> = iced_runtime::task::into_stream(boot(&mut app))
        .map(|stream| block_on(stream.collect()))
        .unwrap_or_default();
    assert!(
        matches!(
            actions.as_slice(),
            [Action::Output(AppMessage::Capture(
                capture::Message::Start(CaptureMode::Selection)
            ))]
        ),
        "{actions:?}"
    );
    assert!(
        iced_runtime::task::into_stream(boot(&mut app)).is_none(),
        "only once"
    );
}

#[test]
fn requests_arrive_through_the_subscription() {
    let (mut app, _fake) = App::for_test();
    assert!(into_recipes(subscription(&app)).is_empty());

    let (sender, requests) = event::channel();
    app.ipc = State::new(Some(requests), None);
    let path = absolute("picture.png");
    let (request, _answer) = Request::new(Command::Open(path.clone()));
    assert!(sender.send(request));

    let mut recipes = into_recipes(subscription(&app));
    assert_eq!(recipes.len(), 1);
    let mut messages = recipes
        .pop()
        .unwrap()
        .stream(futures::stream::empty().boxed());
    match block_on(messages.next()) {
        Some(AppMessage::Ipc(Message::Received(request))) => {
            assert_eq!(request.command(), &Command::Open(path));
        }
        other => panic!("expected a received request, got {other:?}"),
    }
}
