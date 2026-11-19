use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use rumqttc::{Client, Event, MqttOptions, Packet, QoS};
use serde_json::Value;

use super::{cache_dir, device_name, diag_on, fetch_box, local_time, msg_of, received, subscribed, BoxArgs, Bus, Conn, DiagArgs, Msg};

pub struct Mqtt {
    client: Client,
    rx: mpsc::Receiver<Msg>,
    topic: String,
    start: Instant,
}

pub fn connect(c: &Conn, ieee: &str) -> Result<Mqtt> {
    let broker = c.broker.clone().or_else(|| std::env::var("MQTT_BROKER").ok()).unwrap_or_else(|| "localhost".into());
    let mut o = MqttOptions::new(format!("outglow-diag-{}", std::process::id()), broker.clone(), c.port);
    o.set_keep_alive(Duration::from_secs(30));
    o.set_max_packet_size(1 << 20, 1 << 20);
    let (client, mut conn) = Client::new(o, 64);
    let devices = format!("{}/bridge/devices", c.base_topic);
    client.subscribe(&devices, QoS::AtMostOnce)?;
    let until = Instant::now() + Duration::from_secs_f64(c.timeout.clamp(1.0, 10.0));
    let mut name = None;
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        match conn.recv_timeout(left) {
            Ok(Ok(Event::Incoming(Packet::Publish(p)))) if p.topic == devices => {
                name = device_name(&p.payload, ieee);
                break;
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => anyhow::bail!("MQTT broker {broker}: {e}"),
            Err(_) => break,
        }
    }
    client.unsubscribe(&devices)?;
    let topic = format!("{}/{}", c.base_topic, name.as_deref().unwrap_or(ieee));
    let (tx, rx) = mpsc::channel();
    let t = topic.clone();
    std::thread::spawn(move || {
        for ev in conn.iter() {
            if let Some(m) = msg_of(ev, &t, &broker) {
                let last = matches!(m, Msg::Failed(_));
                if tx.send(m).is_err() || last {
                    return;
                }
            }
        }
    });
    client.subscribe(&topic, QoS::AtMostOnce)?;
    let mut m = Mqtt { client, rx, topic, start: Instant::now() };
    subscribed(&mut m, c.timeout)?;
    Ok(m)
}

impl Bus for Mqtt {
    fn topic(&self) -> &str {
        &self.topic
    }

    fn next(&mut self, wait: Duration) -> Result<Option<Msg>> {
        received(self.rx.recv_timeout(wait))
    }

    fn set(&mut self, v: Value) -> Result<()> {
        self.client.publish(format!("{}/set", self.topic), QoS::AtMostOnce, false, v.to_string())?;
        Ok(())
    }

    fn clock(&self) -> Duration {
        self.start.elapsed()
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

pub fn run_diag(a: DiagArgs) -> Result<()> {
    let cache = cache_dir(std::env::var_os("HOME"))?;
    let mut m = connect(&a.conn, &a.ieee)?;
    let r = diag_on(&mut m, &a, &cache, &mut std::io::stdout(), local_time);
    let _ = m.client.disconnect();
    r
}

pub fn read_box(a: &BoxArgs, ieee: &str) -> Result<(Vec<u8>, Vec<usize>, Option<PathBuf>)> {
    let mut m = connect(&a.conn, ieee)?;
    let (blob, missing) = fetch_box(&mut m, a.conn.timeout.max(240.0))?;
    let _ = m.client.disconnect();
    let save = match &a.json {
        Some(p) => p.clone(),
        None => cache_dir(std::env::var_os("HOME"))?.join(format!("{ieee}-box-{}.json", local_time("%Y%m%d-%H%M%S"))),
    };
    Ok((blob, missing, Some(save)))
}
