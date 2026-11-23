use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use tempfile::tempdir;
use vug::fetch::{FetchError, fetch, sha256_file};

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

struct Server {
    url: String,
    requests: Arc<AtomicUsize>,
}

fn serve(status: u16, body: Vec<u8>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            let complete = loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break false,
                    Ok(n) => {
                        request.extend_from_slice(&chunk[..n]);
                        if request.windows(4).any(|w| w == b"\r\n\r\n") {
                            break true;
                        }
                    }
                    Err(_) => break false,
                }
            };
            if !complete {
                continue;
            }
            counter.fetch_add(1, Ordering::SeqCst);
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            if stream.write_all(header.as_bytes()).is_err()
                || stream.write_all(&body).is_err()
                || stream.flush().is_err()
            {
                continue;
            }
        }
    });
    Server {
        url: format!("http://127.0.0.1:{port}/file"),
        requests,
    }
}

#[test]
fn sha256_file_empty() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("empty");
    fs::write(&file, b"").unwrap();
    assert_eq!(sha256_file(&file).unwrap(), EMPTY_SHA256);
}

#[test]
fn sha256_file_abc() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("abc");
    fs::write(&file, b"abc").unwrap();
    assert_eq!(sha256_file(&file).unwrap(), ABC_SHA256);
}

#[test]
fn fetch_returns_cache_path_with_body() {
    let dir = tempdir().unwrap();
    let cache = dir.path().join("cache");
    let server = serve(200, b"abc".to_vec());
    let path = fetch(&server.url, ABC_SHA256, &cache).unwrap();
    assert_eq!(path, cache.join(ABC_SHA256));
    assert_eq!(fs::read(&path).unwrap(), b"abc");
    assert!(!cache.join(format!("{ABC_SHA256}.partial")).exists());
}

#[test]
fn fetch_cache_hit_makes_no_second_request() {
    let dir = tempdir().unwrap();
    let cache = dir.path().join("cache");
    let server = serve(200, b"abc".to_vec());
    fetch(&server.url, ABC_SHA256, &cache).unwrap();
    fetch(&server.url, ABC_SHA256, &cache).unwrap();
    assert_eq!(server.requests.load(Ordering::SeqCst), 1);
}

#[test]
fn fetch_hash_mismatch_removes_partial() {
    let dir = tempdir().unwrap();
    let cache = dir.path().join("cache");
    let server = serve(200, b"abc".to_vec());
    match fetch(&server.url, EMPTY_SHA256, &cache) {
        Err(FetchError::HashMismatch { expected, actual }) => {
            assert_eq!(expected, EMPTY_SHA256);
            assert_eq!(actual, ABC_SHA256);
        }
        other => panic!("expected HashMismatch, got {other:?}"),
    }
    assert!(fs::read_dir(&cache).unwrap().next().is_none());
}

#[test]
fn fetch_status_404_is_http_error() {
    let dir = tempdir().unwrap();
    let cache = dir.path().join("cache");
    let server = serve(404, b"not found".to_vec());
    match fetch(&server.url, ABC_SHA256, &cache) {
        Err(FetchError::Http(_)) => {}
        other => panic!("expected Http error, got {other:?}"),
    }
    assert!(fs::read_dir(&cache).unwrap().next().is_none());
}

#[test]
fn fetch_connection_refused_is_http_error() {
    let dir = tempdir().unwrap();
    let cache = dir.path().join("cache");
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let url = format!("http://127.0.0.1:{port}/file");
    match fetch(&url, ABC_SHA256, &cache) {
        Err(FetchError::Http(_)) => {}
        other => panic!("expected Http error, got {other:?}"),
    }
    assert!(fs::read_dir(&cache).unwrap().next().is_none());
}

#[test]
fn fetch_replaces_cache_file_with_wrong_content() {
    let dir = tempdir().unwrap();
    let cache = dir.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join(ABC_SHA256), b"stale").unwrap();
    let server = serve(200, b"abc".to_vec());
    let path = fetch(&server.url, ABC_SHA256, &cache).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"abc");
    assert_eq!(server.requests.load(Ordering::SeqCst), 1);
}

#[test]
fn fetch_creates_missing_cache_dir() {
    let dir = tempdir().unwrap();
    let cache = dir.path().join("one").join("two");
    let server = serve(200, b"abc".to_vec());
    let path = fetch(&server.url, ABC_SHA256, &cache).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"abc");
}

#[test]
fn fetch_large_body() {
    let dir = tempdir().unwrap();
    let body = vec![0xabu8; 12 * 1024 * 1024];
    let body_file = dir.path().join("body");
    fs::write(&body_file, &body).unwrap();
    let sha = sha256_file(&body_file).unwrap();
    let cache = dir.path().join("cache");
    let server = serve(200, body.clone());
    let path = fetch(&server.url, &sha, &cache).unwrap();
    assert_eq!(fs::read(&path).unwrap(), body);
}

#[test]
fn fetch_sha512_rejects_wrong_tarball_without_publishing_it() {
    let dir = tempdir().unwrap();
    let server = serve(200, b"corrupt archive".to_vec());
    let expected = concat!(
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a",
        "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );
    let err = vug::fetch::fetch_sha512(&server.url, expected, dir.path()).unwrap_err();
    assert!(matches!(err, FetchError::HashMismatch { expected: hash, .. } if hash == expected));
    assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
}
