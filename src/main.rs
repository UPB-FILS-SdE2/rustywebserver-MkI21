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

use std::collections::HashMap;


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


    let mut headers = HashMap::new();
    for line in &lines[1..] {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_string(), value.trim().to_string());
        }
    }

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

    // Combine query string and POST data
    let combined_query = combine_query_and_post_data(query_string, post_data.as_deref());


    // Check if the requested file exists
    if !file_path.exists() {
        let status_code = "404";
        let status_text = "Not Found";
        send_response(
            &mut stream,
            http_version,
            status_code,
            status_text,
            "text/plain",
            "<html>404 Not Found</html>",
        )
        .await?;
        log_connection(method, &stream, requested_path, status_code, status_text).await;
        return Ok(());
    }

    // Check if the requested file is forbidden
    if is_forbidden_file(&file_path, &root_folder) {
        send_response(
            &mut stream,
            http_version,
            "403",
            "Forbidden",
            "text/plain; charset=utf-8",
            "<html>403 Forbidden</html>",
        )
        .await?;
        log_connection(method, &stream, requested_path, "403", "Forbidden").await;
        return Ok(());
    }

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
            send_response(
                &mut stream,
                http_version,
                "403",
                "Forbidden",
                "text/plain; charset=utf-8",
                "<html>403 Forbidden</html>",
            )
            .await?;
            log_connection(method, &stream, requested_path, "403", "Forbidden").await;
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
                    send_response(
                        &mut stream,
                        http_version,
                        status_code,
                        status_text,
                        "text/plain",
                        "<html>500 Internal Server Error</html>",
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
                    send_response(
                        &mut stream,
                        http_version,
                        status_code,
                        status_text,
                        "text/html; charset=utf-8",
                        &html,
                    )
                    .await?;
                    log_connection(method, &stream, requested_path, status_code, status_text).await;
                    return Ok(());
                }
                Err(_) => {
                    let status_code = "500";
                    let status_text = "Internal Server Error";
                    send_response(
                        &mut stream,
                        http_version,
                        status_code,
                        status_text,
                        "text/plain",
                        "<html>500 Internal Server Error</html>",
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
                    send_response(
                        &mut stream,
                        http_version,
                        status_code,
                        status_text,
                        "text/plain",
                        "<html>404 Not Found</html>",
                    )
                    .await?;
                    log_connection(method, &stream, requested_path, status_code, status_text).await;
                    return Ok(());
                }
            }
        } else {
            let status_code = "404";
            let status_text = "Not Found";
            send_response(
                &mut stream,
                http_version,
                status_code,
                status_text,
                "text/plain",
                "<html>404 Not Found</html>",
            )
            .await?;
            log_connection(method, &stream, requested_path, status_code, status_text).await;
            return Ok(());
        }
    }

    // If the method is not GET or POST, return 405 Method Not Allowed
    let status_code = "405";
    let status_text = "Method Not Allowed";
    send_response(
        &mut stream,
        http_version,
        status_code,
        status_text,
        "text/plain",
        "<html>405 Method Not Allowed</html>",
    )
    .await?;
    log_connection(method, &stream, requested_path, status_code, status_text).await;
    Ok(())
}





fn is_forbidden_file(file_path: &Path, root_folder: &Path) -> bool {
    // Check if the file is outside the root folder (path traversal protection)

    if let Ok(canonical_path) = file_path.canonicalize() {
        if !canonical_path.starts_with(root_folder) {
            return true;
        }
    } else {
        return true;
    }

    // Define specific forbidden directories or files

    let forbidden_patterns = vec![
        "forbidden", // Forbidden directory
        "restricted_area.txt", // Specific file
        "scripts/../", // Example of a path pattern
    ];

    // Check if the file path contains any forbidden patterns

    for pattern in &forbidden_patterns {
        if file_path.to_str().map_or(false, |s| s.contains(pattern)) {
            return true;
        }
    }

    // Additional example: block access to hidden files (starting with '.')

    if file_path
        .file_name()
        .and_then(|name| name.to_str())
        .map_or(false, |s| s.starts_with('.'))
    {
        return true;
    }

    // Example: Check if the file is in a forbidden directory (adjust as needed)

    if let Some(parent) = file_path.parent() {
        if forbidden_patterns
            .iter()
            .any(|pattern| parent.ends_with(pattern))
        {
            return true;
        }
    }

    false
}


// New function to combine query string and POST data
fn combine_query_and_post_data(
    query_string: Option<&str>,
    post_data: Option<&str>,
) -> String {
    let mut combined = String::new();

    if let Some(query) = query_string {
        combined.push_str(query);
    }

    if let Some(post) = post_data {
        if !combined.is_empty() {
            combined.push('&');
        }
        combined.push_str(post);
    }

    combined
}

async fn execute_script(
    script_path: PathBuf,
    stream: &mut TcpStream,
    http_version: &str,
    headers: &HashMap<String, String>,
    method: &str,
    requested_path: &str,
    query_string: Option<&str>, // Optional query string
    combined_query: Option<&str>,    // Optional POST data
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

    // Add POST data if present
    if method == "POST" {
        if let Some(data) = combined_query {
            for param in data.split('&') {
                if let Some((key, value)) = param.split_once('=') {
                    let var_name = format!("Query_{}", key);
                    env_vars.insert(var_name, value.to_string());
                }
            }
        }
    }

    if let Some(query_str) = combined_query {
        for param in query_str.split('&') {
            if let Some((key, value)) = param.split_once('=') {
                let var_name = format!("Query_{}", key);
                env_vars.insert(var_name, value.to_string());
            }
        }
    }

    // Execute the script
    let mut command = Command::new(&script_path);

    // Set environment variables for the command
    for (key, value) in env_vars {
        command.env(key, value);
    }

    // Capture output (stdout and stderr)
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
    let response = format!("{}\r\n\r\n{}", response_headers.join("\r\n"), body);

    // Send the response
    stream.write_all(response.as_bytes()).await?;

    Ok((status_code, status_text))
}


async fn send_response(

    stream: &mut TcpStream,

    http_version: &str,

    status_code: &str,

    status_text: &str,

    content_type: &str,

    body: &str,

) -> io::Result<()> {

    let response = format!(

        "{} {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",

        http_version,

        status_code,

        status_text,

        content_type,

        body.len(),

        body

    );

    stream.write_all(response.as_bytes()).await

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
            "{} {} {} -> {} ({})",
            method, remote_ip, requested_path, status_code, status_text
        );
    } else {
        println!(
            "{} unknown {} -> {} ({})",
            method, requested_path, status_code, status_text
        );
    }
}