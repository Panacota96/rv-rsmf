//! The synthetic Teams and CSV samples under examples/sample/evidence (invented people, example.com
//! addresses and +57 300 000 000x / +34 600 000 00x numbers; no real data). `files()` is the single source:
//! `make_samples` writes it, `parsers_teams_csv` checks the committed files still match it.
#![allow(dead_code)]

use super::make_msg::{Msg, msg_bytes};
use chrono::{TimeZone, Utc};

/// A 1×1 transparent PNG.
pub const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0x60, 0x60, 0xF8, 0x5F,
    0x0F, 0x00, 0x02, 0x87, 0x01, 0x80, 0xEB, 0x47, 0xBA, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn some(s: &str) -> Option<String> {
    Some(s.to_string())
}

pub fn teams_messages() -> Vec<(&'static str, Msg)> {
    let ana = ("Ana Ruiz", "ana.ruiz@example.com");
    let bruno = ("Bruno Díaz", "bruno.diaz@example.com");
    let carla = ("Carla Gómez", "carla.gomez@example.com");
    let r = |v: &[(&str, &str)]| -> Vec<(String, String)> {
        v.iter()
            .map(|(n, e)| (n.to_string(), e.to_string()))
            .collect()
    };
    let topic = some("Proyecto Faro / General");
    vec![
        (
            "0001.msg",
            Msg {
                class: "IPM.SkypeTeams.Message".into(),
                topic: topic.clone(),
                sender_name: some(ana.0),
                sender_smtp: some(ana.1),
                display_to: some("Bruno Díaz; Carla Gómez"),
                recipients: r(&[bruno, carla]),
                body: some("Buenos días equipo, ¿revisamos hoy el cronograma del piloto?"),
                submit_time: Some(Utc.with_ymd_and_hms(2026, 3, 2, 9, 15, 0).unwrap()),
                message_id: some("<msg-0001@teams.example.com>"),
                ..Default::default()
            },
        ),
        (
            "0002.msg",
            Msg {
                class: "IPM.SkypeTeams.Message".into(),
                topic: topic.clone(),
                sender_name: some(bruno.0),
                sender_smtp: some(bruno.1),
                display_to: some("Ana Ruiz; Carla Gómez"),
                recipients: r(&[ana, carla]),
                html: some(
                    "<html><body><div><p>Sí, adjunto la agenda.</p><p>Hora propuesta: 15:00 &amp; sala 2</p></div></body></html>",
                ),
                submit_time: Some(Utc.with_ymd_and_hms(2026, 3, 2, 9, 20, 30).unwrap()),
                message_id: some("<msg-0002@teams.example.com>"),
                in_reply_to: some("<msg-0001@teams.example.com>"),
                attachments: vec![(
                    "agenda-piloto.txt".into(),
                    b"Agenda (ficticia)\n1. Cronograma\n2. Riesgos\n".to_vec(),
                )],
                ..Default::default()
            },
        ),
        (
            "0003.msg",
            Msg {
                class: "IPM.SkypeTeams.SystemMessage".into(),
                topic: topic.clone(),
                sender_name: some(carla.0),
                sender_smtp: some(carla.1),
                display_to: some("Ana Ruiz; Bruno Díaz"),
                body: some("Carla Gómez cambió el nombre del canal a General."),
                // Only the delivery time: exercises the fallback.
                delivery_time: Some(Utc.with_ymd_and_hms(2026, 3, 2, 9, 25, 0).unwrap()),
                message_id: some("<msg-0003@teams.example.com>"),
                ..Default::default()
            },
        ),
        (
            "0004.msg",
            Msg {
                class: "IPM.Note".into(),
                subject: some("Factura ficticia 0001"),
                sender_name: some("Facturación"),
                sender_smtp: some("facturacion@example.com"),
                display_to: some("Ana Ruiz"),
                body: some("Correo de ejemplo: no es un mensaje de Teams."),
                submit_time: Some(Utc.with_ymd_and_hms(2026, 3, 2, 10, 0, 0).unwrap()),
                ..Default::default()
            },
        ),
    ]
}

fn utf16le_bom(s: &str) -> Vec<u8> {
    let mut v = vec![0xFF, 0xFE];
    v.extend(s.encode_utf16().flat_map(|u| u.to_le_bytes()));
    v
}

pub const XRY_LIKE_CSV: &str = "Index,Time,Chat ID,Source,From,To,Direction,Body,Attachment(s),Deleted\r\n\
1,14/03/2026 10:02:11,chat-001,WhatsApp,Laura Pérez <+57 300 000 0001>,+57 300 000 0002,Outgoing,\"Hola, ¿recibiste el informe?\",,No\r\n\
2,14/03/2026 10:05:40,chat-001,WhatsApp,+57 300 000 0002,Laura Pérez <+57 300 000 0001>,Incoming,\"Sí, ya lo tengo.\r\nTe envío la foto.\",,No\r\n\
3,14/03/2026 10:06:02,chat-001,WhatsApp,+57 300 000 0002,Laura Pérez <+57 300 000 0001>,Incoming,,files/IMG-20260314-0001.jpg,No\r\n\
4,31/02/2026 11:00:00,chat-001,WhatsApp,Laura Pérez <+57 300 000 0001>,+57 300 000 0002,Outgoing,Fecha imposible en el origen,,No\r\n\
5,15/03/2026 08:30:00,chat-002,SMS,Laura Pérez <+57 300 000 0001>,+57 300 000 0003,Outgoing,Reunión confirmada para el lunes.,,Yes\r\n";

pub const GENERIC_CSV: &str = "fecha,chat,remitente,numero,destinatarios,texto,sentido,app,adjunto
2026-10-24 22:10:00,Grupo obra,Marta Ríos,+34 600 000 001,Pablo Sanz;Lucía Vega,¿Mañana a las 9 en la obra?,saliente,Signal,
2026-10-25 02:30:00,Grupo obra,Pablo Sanz,+34 600 000 002,Marta Ríos;Lucía Vega,Vale. Llevo las llaves.,entrante,Signal,
2026-10-25 09:05:00,Grupo obra,Lucía Vega,+34 600 000 003,Marta Ríos;Pablo Sanz,Adjunto el plano revisado.,entrante,Signal,media/plano-0001.png
";

pub const GENERIC_MAP: &str = r#"# Column mapping for generic.csv (synthetic sample), found automatically as <csv stem>.map.toml. Times are wall-clock times in Spain:
# convert with --timezone Europe/Madrid. 2026-10-25 02:30 happens twice there (end of summer time).
[columns]
timestamp = "fecha"
timestamp_format = "%Y-%m-%d %H:%M:%S"
conversation = "chat"
sender = "remitente"
sender_number = "numero"
recipients = "destinatarios"
body = "texto"
direction = "sentido"
application = "app"
attachment = "adjunto"

[direction]
incoming = ["entrante"]
outgoing = ["saliente"]

[options]
delimiter = ","
encoding = "utf-8"
skip_rows = 0
"#;

/// Every sample file: (path relative to examples/sample/evidence, bytes).
pub fn files() -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = teams_messages()
        .into_iter()
        .map(|(n, m)| (format!("teams-msg/{n}"), msg_bytes(&m)))
        .collect();
    v.push(("csv/xry-like.csv".into(), utf16le_bom(XRY_LIKE_CSV)));
    v.push(("csv/generic.csv".into(), GENERIC_CSV.as_bytes().to_vec()));
    v.push((
        "csv/generic.map.toml".into(),
        GENERIC_MAP.as_bytes().to_vec(),
    ));
    v.push(("csv/media/plano-0001.png".into(), PNG_1X1.to_vec()));
    v
}
