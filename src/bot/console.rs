//! stdin input task (mirrors the input thread / CGHost::m_InputMessage in C++ ghost.cpp).
//! Each input line is wrapped into a BotEvent::ConsoleInput and sent to BotCore.

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::messages::BotEvent;

pub fn spawn(event_tx: mpsc::Sender<BotEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim().to_string();

            if line.is_empty() {
                continue;
            }

            if event_tx.send(BotEvent::ConsoleInput(line)).await.is_err() {
                break;
            }
        }
    })
}
