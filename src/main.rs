use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::io::AsyncWriteExt;
use std::env;

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
    println!("Root folder: {}", root_folder.display());
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

    // Read request from stream
    let n = stream.read(&mut buffer).await?;
    if n == 0 {
        return Ok(());
    }

    let request = str::from_utf8(&buffer[..n]).unwrap_or("");

    // Parse request
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
    let file_path = root_folder.join(parts[1].trim_start_matches('/'));
    let http_version = parts[2];

    // Check if the file exists and is readable
    if file_path.exists() && file_path.is_file() {
        match fs::metadata(&file_path) {
            Ok(metadata) => {
                if metadata.permissions().readonly() {
                    // Respond with 403 Forbidden if the file is not writable
                    let status_code = "403";
                    let status_text = "Forbidden";
                    stream.write_all(format!("{} {} {}\r\nConnection: close\r\n\r\n", http_version, status_code, status_text).as_bytes()).await?;
                    log_connection(method, &stream, &file_path, status_code, status_text, &root_folder).await;
                    return Ok(());
                } else {
                    // Respond with 200 OK if the file is readable
                    match read_file(&file_path).await {
                        Ok(contents) => {
                            let mime_type = get_mime_type(&file_path);
                            let status_code = "200";
                            let status_text = "OK";
                            let header = format!("{} {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", http_version, status_code, status_text, mime_type, contents.len());
                            stream.write_all(header.as_bytes()).await?;
                            stream.write_all(&contents).await?;
                            log_connection(method, &stream, &file_path, status_code, status_text, &root_folder).await;
                            return Ok(());
                        }
                        Err(_) => {
                            let status_code = "404";
                            let status_text = "Not Found";
                            stream.write_all(format!("{} {} {}\r\nConnection: close\r\n\r\n", http_version, status_code, status_text).as_bytes()).await?;
                            log_connection(method, &stream, &file_path, status_code, status_text, &root_folder).await;
                            return Ok(());
                        }
                    }
                }
            }
            Err(_) => {
                let status_code = "500";
                let status_text = "Internal Server Error";
                stream.write_all(format!("{} {} {}\r\nConnection: close\r\n\r\n", http_version, status_code, status_text).as_bytes()).await?;
                log_connection(method, &stream, &file_path, status_code, status_text, &root_folder).await;
                return Ok(());
            }
        }
    } else {
        let status_code = "404";
        let status_text = "Not Found";
        stream.write_all(format!("{} {} {}\r\nConnection: close\r\n\r\n", http_version, status_code, status_text).as_bytes()).await?;
        log_connection(method, &stream, &file_path, status_code, status_text, &root_folder).await;
        return Ok(());
    }
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

async fn log_connection(
    method: &str,
    stream: &TcpStream,
    file_path: &Path,
    status_code: &str,
    status_text: &str,
    root_folder: &Path,
) {
    if let Ok(remote_addr) = stream.peer_addr() {
        let remote_ip = remote_addr.ip().to_string(); // Get the IP address without port

        // Display the path relative to the root folder
        let relative_path = file_path.strip_prefix(root_folder).unwrap_or(file_path).display();

        // Print log entry
        println!("{} {} {} -> {} ({})", method, remote_ip, relative_path, status_code, status_text);
    } else {
        // Print log entry with unknown IP
        println!("{} unknown {} -> {} ({})", method, file_path.display(), status_code, status_text);
    }
}
