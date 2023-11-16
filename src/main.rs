use std::borrow::Cow;
use std::net::SocketAddr;
use std::ops::ControlFlow;
use std::path::PathBuf;
use axum::extract::{ConnectInfo, WebSocketUpgrade};
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use axum::response::IntoResponse;
use axum::{Router, ServiceExt};
use axum::routing::get;
use axum_extra::TypedHeader;
use futures_util::{SinkExt, StreamExt};
use tower_http::services::ServeDir;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "axum_websocket=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
    // 将前端加入到程序中
    let assets_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
    // 创建axum router
    let app = Router::new()
        .fallback_service(ServeDir::new(assets_dir).append_index_html_on_directories(true))
        .route("/ws", get(ws_handler))
        .layer(TraceLayer::new_for_http().make_span_with(DefaultMakeSpan::default().include_headers(true)));
    // 创建tcp binding
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    // 创建axum服务
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    user_agent: Option<TypedHeader<headers::UserAgent>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    // 获取用户代理
    let user_agent = if let Some(TypedHeader(user_agent)) = user_agent {
        user_agent.to_string()
    } else {
        String::from("unknown browser")
    };
    println!("user_agent {} at addr:{} connected", user_agent, addr);
    ws.on_upgrade(move |socket| {
        handle_socket(socket, addr)
    })
}

/// 创立socket连接
async fn handle_socket(mut socket: WebSocket, addr: SocketAddr) {
    // send 一个 ping 信息，查看是否连接成功
    if socket.send(Message::Ping(vec![1, 2, 3])).await.is_ok() {
        println!("ping {} complete", addr.to_string());
    } else {
        println!("connection refuse !");
        return;
    }
    // 接受socket信息
    if let Some(msg) = socket.recv().await {
        if let Ok(msg) = msg {
            // 收到关闭连接信息
            if process_message(msg, addr).is_break() {
                return;
            }
        } else {
            // msg 收到有误
            println!("{} disconnected", addr);
            return;
        }
    }
    // 等待client做响应
    for i in 0..5 {
        if socket.send(Message::Text(
            format!("hello {} times!", i)))
            .await.is_err() {
            // client未响应
            println!("{} disconnected", addr);
        }
        // 等待100ms
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    // 将发送方与接收方分开做处理
    let (mut sender, mut receiver) = socket.split();
    // 处理发送方信息
    let mut send_task = tokio::spawn(async move {
        // 想客户端推送信息
        let n_msg = 20;
        for i in 0..n_msg {
            if sender.send(Message::Text(format!("Server message {} ..", i)))
                .await.is_err() {
                return i;
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        println!("send close to {} ", addr);
        if let Err(e) = sender.send(Message::Close(Some(CloseFrame {
            code: axum::extract::ws::close_code::NORMAL,
            reason: Cow::from("bye"),
        }))).await {
            println!("could not send close message to {}", e);
        }
        n_msg
    });
    // 处理接受方信息
    let mut recv_task = tokio::spawn(async move {
        let mut cnt = 0;
        while let Some(Ok(msg)) = receiver.next().await {
            cnt += 1;
            if process_message(msg, addr).is_break() {
                break;
            }
        }
        cnt
    });
    // 如果有一方退出了，另一方也退出
    tokio::select! {
        rv_a = (&mut send_task) => {
            match rv_a {
                Ok(a) => println!("{} message send to {}",a,addr),
                Err(a) => println!("Error sending message {}",a)
            }
            recv_task.abort();
        },
       rv_b = (&mut recv_task) => {
            match rv_b {
                Ok(b) => println!("received {} message",b),
                Err(b) => println!("Error receiving message {}",b)
            }
            send_task.abort();
        },
    }
    println!("context {} destroyed", addr);
}

/// 处理socket信息,返回控制流
fn process_message(msg: Message, addr: SocketAddr) -> ControlFlow<(), ()> {
    match msg {
        Message::Text(t) => {
            println!("{} sent text:{}", addr, t);
        }
        Message::Binary(b) => {
            println!("{} sent bytes:{}", addr, b.len());
        }
        Message::Ping(p) => {
            println!("{} send ping with {:?}", addr, p);
        }
        Message::Pong(p) => {
            println!("{} send pong with {:?}", addr, p);
        }
        Message::Close(c) => {
            if let Some(c) = c {
                println!("{} send close with code {} and reason {}", addr, c.code, c.reason);
            } else {
                println!("{} send close without reason", addr);
            }
            return ControlFlow::Break(());
        }
    }
    ControlFlow::Continue(())
}