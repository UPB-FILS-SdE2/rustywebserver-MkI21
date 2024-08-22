use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;

#[tokio::main]
async fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: rustywebserver PORT ROOT_FOLDER");
        std::process::exit(1);
    }

    let port = args[1].parse::<u16>().expect("Invalid port number");
    let root_folder = PathBuf::from(&args[2])
        .canonicalize()
        .expect("Invalid root folder path");

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

async fn handle_request(mut stream: TcpStream, root_folder: PathBuf) -> io::Result<()> {
    let mut buffer = [0; 4096];
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

    // Collect headers
    let mut headers = HashMap::new();
    for line in &lines[1..] {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_string(), value.trim().to_string());
        }
    }

    // Handle POST requests
    if method == "POST" {
        let mut content_length: usize = 0;
        if let Some(len) = headers.get("Content-Length") {
            content_length = len.parse().unwrap_or(0);
        }

        let mut post_data = vec![0; content_length];
        stream.read_exact(&mut post_data).await?;

        let response = format!(
            "{} 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            http_version,
            post_data.len(),
            String::from_utf8_lossy(&post_data)
        );
        stream.write_all(response.as_bytes()).await?;
        log_connection(method, &stream, requested_path, "200", "OK").await;

        return Ok(());
    }

    // Handle GET requests
    if method == "GET" {
        // Check for forbidden access
        if file_path.starts_with(root_folder.join("forbidden")) {
            let status_code = "403";
            let status_text = "Forbidden";
            stream
                .write_all(
                    format!(
                        "{} {} {}\r\nConnection: close\r\n\r\n",
                        http_version, status_code, status_text
                    )
                    .as_bytes(),
                )
                .await?;
            log_connection(method, &stream, requested_path, status_code, status_text).await;
            return Ok(());
        }

        // Execute scripts
        if file_path.starts_with(root_folder.join("scripts")) && file_path.is_file() {
            let (status_code, status_text) =
                match execute_script(file_path, &mut stream, http_version, &headers, method, requested_path).await {
                    Ok((status_code, status_text)) => (status_code, status_text),
                    Err(e) => {
                        let status_code = "500";
                        let status_text = "Internal Server Error";
                        stream
                            .write_all(
                                format!(
                                    "{} {} {}\r\nConnection: close\r\n\r\n",
                                    http_version, status_code, status_text
                                )
                                .as_bytes(),
                            )
                            .await?;
                        (status_code, status_text)
                    }
                };
            log_connection(method, &stream, requested_path, status_code, status_text).await;
            return Ok(());
        }

        // Serve files and directories
        if file_path.is_dir() {
            match generate_directory_listing(&file_path, &root_folder).await {
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
                    stream
                        .write_all(
                            format!(
                                "{} {} {}\r\nConnection: close\r\n\r\n",
                                http_version, status_code, status_text
                            )
                            .as_bytes(),
                        )
                        .await?;
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
                    let header = format!(
                        "{} {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        http_version, status_code, status_text, mime_type, contents.len()
                    );
                    stream.write_all(header.as_bytes()).await?;
                    stream.write_all(&contents).await?;
                    log_connection(method, &stream, requested_path, status_code, status_text).await;
                    return Ok(());
                }
                Err(_) => {
                    let status_code = "404";
                    let status_text = "Not Found";
                    stream
                        .write_all(
                            format!(
                                "{} {} {}\r\nConnection: close\r\n\r\n",
                                http_version, status_code, status_text
                            )
                            .as_bytes(),
                        )
                        .await?;
                    log_connection(method, &stream, requested_path, status_code, status_text).await;
                    return Ok(());
                }
            }
        } else {
            let status_code = "404";
            let status_text = "Not Found";
            stream
                .write_all(
                    format!(
                        "{} {} {}\r\nConnection: close\r\n\r\n",
                        http_version, status_code, status_text
                    )
                    .as_bytes(),
                )
                .await?;
            log_connection(method, &stream, requested_path, status_code, status_text).await;
            return Ok(());
        }
    }

    // If the method is not GET or POST, return 405 Method Not Allowed
    let status_code = "405";
    let status_text = "Method Not Allowed";
    stream
        .write_all(
            format!(
                "{} {} {}\r\nConnection: close\r\n\r\n",
                http_version, status_code, status_text
            )
            .as_bytes(),
        )
        .await?;
    log_connection(method, &stream, requested_path, status_code, status_text).await;
    Ok(())
}


use std::collections::HashMap;

async fn execute_script(
    script_path: PathBuf,
    stream: &mut TcpStream,
    http_version: &str,
    headers: &HashMap<String, String>,
    method: &str,
    requested_path: &str,
) -> io::Result<(&'static str, &'static str)> {
    // Prepare environment variables
    let mut env_vars = HashMap::new();
    for (key, value) in headers {
        env_vars.insert(key.clone(), value.clone());
    }
    env_vars.insert("Method".to_string(), method.to_string());
    env_vars.insert("Path".to_string(), requested_path.to_string());

    // Execute the script
    let mut command = Command::new(&script_path);
    
    // Set environment variables
    for (key, value) in env_vars {
        command.env(key, value);
    }

    // Capture both stdout and stderr
    let output = command.output().await?;

    let status_code = if output.status.success() {
        "200"
    } else {
        "500"
    };
    let status_text = if output.status.success() {
        "OK"
    } else {
        "Internal Server Error"
    };

    let mut response_headers = vec![
        format!("{} {} {}", http_version, status_code, status_text),
        "Connection: close".to_string(),
    ];

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.is_empty() {
            // The first empty line indicates the end of headers
            break;
        }
        response_headers.push(line.to_string());
    }

    let body_start = stdout.find("\n\n").unwrap_or(0) + 2;
    let body = &stdout[body_start..];

    // Prepare the full response
    let response = format!(
        "{}\r\n\r\n{}",
        response_headers.join("\r\n"),
        body
    );

    // Send the response
    stream.write_all(response.as_bytes()).await?;

    Ok((status_code, status_text))
}
















async fn generate_directory_listing(path: &Path, root_folder: &Path) -> io::Result<String> {
    let mut html = String::from("<html><body><h1>Directory listing</h1><ul>");
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_str()
            .unwrap_or_default();
        let relative_path = path.strip_prefix(root_folder).unwrap_or(&path);
        html.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>",
            relative_path.display(),
            filename
        ));
    }
    html.push_str("</ul></body></html>");
    Ok(html)
}





async fn read_file(path: &Path) -> io::Result<Vec<u8>> {
    fs::read(path)
}

fn get_mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("txt") => "text/plain; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
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
        println!(
            "{} {} /{} -> {} ({})",
            method, remote_ip, requested_path, status_code, status_text
        );
    } else {
        println!(
            "{} unknown {} -> {} ({})",
            method, requested_path, status_code, status_text
        );
    }
}
