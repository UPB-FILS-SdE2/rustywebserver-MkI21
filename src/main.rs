use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
    let full_path = parts[1];
    let http_version = parts[2];

    // Extract requested path and query string
    let (requested_path, query_string) = if let Some(idx) = full_path.find('?') {
        (&full_path[..idx], Some(&full_path[idx + 1..]))
    } else {
        (full_path, None)
    };

    let file_path = root_folder.join(requested_path.trim_start_matches('/'));

    // Collect headers
    let mut headers = HashMap::new();
    for line in &lines[1..] {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_string(), value.trim().to_string());
        }
    }

    // Prepare to capture POST data
    let mut post_data: Option<String> = None;

    // Handle POST requests
    if method == "POST" {
        let mut content_length: usize = 0;
        if let Some(len) = headers.get("Content-Length") {
            content_length = len.parse().unwrap_or(0);
        }

        let mut data = vec![0; content_length];
        stream.read_exact(&mut data).await?;
        post_data = Some(String::from_utf8_lossy(&data).to_string());
    }

    // Handle GET and POST requests
    if method == "GET" || method == "POST" {
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
            let (status_code, status_text) = match execute_script(
                file_path,
                &mut stream,
                http_version,
                &headers,
                method,
                requested_path,
                query_string,
                post_data.as_deref(),
            )
            .await
            {
                Ok((status_code, status_text)) => (status_code, status_text),
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
                    stream
                        .write_all(
                            format!(
                                "{} {} {}\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n{}",
                                http_version, status_code, status_text, html
                            )
                            .as_bytes(),
                        )
                        .await?;
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



async fn execute_script(
    script_path: PathBuf,
    stream: &mut TcpStream,
    http_version: &str,
    headers: &HashMap<String, String>,
    method: &str,
    requested_path: &str,
    query_string: Option<&str>,
    post_data: Option<&str>,
) -> io::Result<(&'static str, &'static str)> {
    // Prepare environment variables
    let mut env_vars = HashMap::new();

    // Add headers as environment variables
    for (key, value) in headers {
        env_vars.insert(key.clone(), value.clone());
    }

    // Add method and path as environment variables
    env_vars.insert("Method".to_string(), method.to_string());
    env_vars.insert("Path".to_string(), requested_path.to_string());

    // Parse query string and add to env_vars
    if let Some(query_str) = query_string {
        for param in query_str.split('&') {
            if let Some((key, value)) = param.split_once('=') {
                let var_name = format!("Query_{}", key);
                env_vars.insert(var_name, value.to_string());
            }
        }
    }

    // Add POST data to environment variables
    if let Some(data) = post_data {
        env_vars.insert("POST_DATA".to_string(), data.to_string());
    }

    // Execute the script
    let mut command = Command::new(script_path);
    command.envs(&env_vars);

    let output = command.output().await?;
    if output.status.success() {
        let response = String::from_utf8_lossy(&output.stdout);
        let response_bytes = response.as_bytes(); // Create a longer-lived reference

        let status_code = "200";
        let status_text = "OK";
        let header = format!(
            "{} {} {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n",
            http_version, status_code, status_text
        );
        stream.write_all(header.as_bytes()).await?;
        stream.write_all(response_bytes).await?; // Use the longer-lived reference
        return Ok((status_code, status_text));
    } else {
        let response = String::from_utf8_lossy(&output.stderr);
        let response_bytes = response.as_bytes(); // Create a longer-lived reference

        let status_code = "500";
        let status_text = "Internal Server Error";
        let header = format!(
            "{} {} {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n",
            http_version, status_code, status_text
        );
        stream.write_all(header.as_bytes()).await?;
        stream.write_all(response_bytes).await?; // Use the longer-lived reference
        return Ok((status_code, status_text));
    }
}



async fn generate_directory_listing(
    dir: &Path,
    root_folder: &Path,
) -> io::Result<String> {
    let mut entries = vec![];
    let root = root_folder.to_string_lossy();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let rel_path = path.strip_prefix(root_folder).unwrap_or(&path);

        let display_path = rel_path.to_string_lossy().replace('\\', "/");
        let display_name = entry.file_name().to_string_lossy();

        if path.is_dir() {
            entries.push(format!("<li><a href=\"{}/\">{}/</a></li>", display_path, display_name));
        } else {
            entries.push(format!("<li><a href=\"{}\">{}</a></li>", display_path, display_name));
        }
    }

    let list = entries.join("\n");

    let html = format!(
        "<html><body><h1>Directory listing for {}</h1><ul>{}</ul></body></html>",
        dir.to_string_lossy(),
        list
    );

    Ok(html)
}

async fn read_file(path: &Path) -> io::Result<Vec<u8>> {
    fs::read(path)
}

fn get_mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|s| s.to_str()) {
        Some("html") => "text/html",
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        _ => "application/octet-stream",
    }
}

async fn log_connection(
    method: &str,
    stream: &TcpStream,
    path: &str,
    status_code: &str,
    status_text: &str,
) {
    if let Ok(peer_addr) = stream.peer_addr() {
        println!(
            "{} {} - {} {} - {}",
            peer_addr,
            method,
            path,
            status_code,
            status_text
        );
    }
}
