use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str;
use std::env;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::AsyncWriteExt;

#[tokio::main]
async fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: rustywebserver PORT ROOT_FOLDER");
        std::process::exit(1);
    }

    let port = args[1].parse::<u16>().expect("Invalid port number");
    let root_folder = PathBuf::from(&args[2]).canonicalize().expect("Invalid root folder path");

    // Print root folder and server listening message once
    println!("Root folder: {:?}", root_folder.display());
    println!("Server listening on 0.0.0.0:{}", port);

    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    
    loop {
        let (stream, _) = listener.accept().await?;
        let root_folder = root_folder.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_request(stream, root_folder).await {
                eprintln!("Error handling request: {}", e);
            }
        });
    }
}

async fn handle_request(stream: TcpStream, root_folder: PathBuf) -> io::Result<()> {
    let mut buffer = [0; 4096];
    let mut stream = stream;

    let n = stream.read(&mut buffer).await?;
    if n == 0 {
        return Ok(());
    }

    let request = str::from_utf8(&buffer[..n]).unwrap_or("");

    let lines: Vec<&str> = request.lines().collect();
    if lines.is_empty() {
        return Ok(());
    }

    let request_line = lines[0];
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 3 {
        return Ok(());
    }

    let method = parts[0];
    let requested_path = parts[1].trim_start_matches('/');
    let file_path = root_folder.join(requested_path);
    let http_version = parts[2];

    // Check for forbidden access
    if file_path.starts_with(root_folder.join("forbidden")) {
        let status_code = "403";
        let status_text = "Forbidden";
        stream.write_all(format!("{} {} {}\r\nConnection: close\r\n\r\n", http_version, status_code, status_text).as_bytes()).await?;
        log_connection(method, &stream, requested_path, status_code, status_text).await;
        return Ok(());
    }

    let _response = if file_path.is_dir() {
        match generate_directory_listing(&file_path).await {
            Ok(html) => {
                let status_code = "200";
                let status_text = "OK";
                stream.write_all(format!("{} {} {}\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n{}", http_version, status_code, status_text, html).as_bytes()).await?;
                log_connection(method, &stream, requested_path, status_code, status_text).await;
                return Ok(());
            }
            Err(_) => {
                let status_code = "500";
                let status_text = "Internal Server Error";
                stream.write_all(format!("{} {} {}\r\nConnection: close\r\n\r\n", http_version, status_code, status_text).as_bytes()).await?;
                log_connection(method, &stream, requested_path, status_code, status_text).await;
                return Ok(());
            }
        }
    } else if file_path.exists() && file_path.is_file() {
        match read_file(&file_path).await {
            Ok(contents) => {
                let mime_type = get_mime_type(&file_path);
                let status_code = "200";
                let status_text = "OK";
                let header = format!("{} {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", http_version, status_code, status_text, mime_type, contents.len());
                stream.write_all(header.as_bytes()).await?;
                stream.write_all(&contents).await?;
                log_connection(method, &stream, requested_path, status_code, status_text).await;
                return Ok(());
            }
            Err(_) => {
                let status_code = "404";
                let status_text = "Not Found";
                stream.write_all(format!("{} {} {}\r\nConnection: close\r\n\r\n", http_version, status_code, status_text).as_bytes()).await?;
                log_connection(method, &stream, requested_path, status_code, status_text).await;
                return Ok(());
            }
        }
    } else {
        let status_code = "404";
        let status_text = "Not Found";
        stream.write_all(format!("{} {} {}\r\nConnection: close\r\n\r\n", http_version, status_code, status_text).as_bytes()).await?;
        log_connection(method, &stream, requested_path, status_code, status_text).await;
        return Ok(());
    };
}

async fn read_file(path: &Path) -> io::Result<Vec<u8>> {
    fs::read(path)
}

fn get_mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("html") => "text/html",
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

async fn generate_directory_listing(path: &Path) -> io::Result<String> {
    let mut html = String::from("<html><body><h1>Directory listing</h1><ul>");
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let filename = path.file_name().unwrap_or_default().to_str().unwrap_or_default();
        html.push_str(&format!("<li><a href=\"{}\">{}</a></li>", filename, filename));
    }
    html.push_str("</ul></body></html>");
    Ok(html)
}

async fn log_connection(
    method: &str,
    stream: &TcpStream,
    requested_path: &str,
    status_code: &str,
    status_text: &str,
) {
    if let Ok(remote_addr) = stream.peer_addr() {
        let remote_ip = remote_addr.ip().to_string();
        println!("{} {} /{} -> {} ({})", method, remote_ip, requested_path, status_code, status_text);
    } else {
        println!("{} unknown {} -> {} ({})", method, requested_path, status_code, status_text);
    }
}