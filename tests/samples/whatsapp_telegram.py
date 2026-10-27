#!/usr/bin/env python3
"""Generates the synthetic WhatsApp and Telegram samples for rv-rsmf (Python 3 standard library only).

Everything here is invented: people, phone numbers (+33 6 00 00 00 0x), places and links (example.org).
No real chat, contact or media is used. The output is deterministic: running the script twice gives
byte-identical files, so the samples' hashes are stable.

Writes:
  examples/sample/evidence/whatsapp-android-en/WhatsApp Chat with Ana Ruiz.txt  (+ IMG-20260302-WA0001.jpg)
  examples/sample/evidence/whatsapp-ios-es.zip         (_chat.txt + 00000003-PHOTO-2026-03-04-21-15-02.jpg)
  examples/sample/evidence/whatsapp-ambiguous.txt      (every day and month <= 12 -> needs --date-order)
  examples/sample/evidence/whatsapp-dst.txt            (Europe/Paris clock changes 2026-03-29 / 2026-10-25)
  examples/sample/evidence/telegram/result.json        (+ photos/, voice_messages/)
  examples/sample/expected/<sample>.json               (what the parser must produce, counted by hand
                                                        from the content below, not by running rv-rsmf)

Usage: python3 tests/samples/whatsapp_telegram.py
"""

import json
import struct
import zipfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
EVIDENCE = REPO / "examples" / "sample" / "evidence"
EXPECTED = REPO / "examples" / "sample" / "expected"

LRM = "‎"  # LEFT-TO-RIGHT MARK (iOS puts it before system text and attachments)
NNBSP = " "  # NARROW NO-BREAK SPACE (iOS 17+ before AM/PM)
NBSP = " "
LRE, PDF = "‪", "‬"  # Android wraps phone numbers in LTR embedding


def tiny_jpeg(comment: str) -> bytes:
    """A valid baseline JPEG: one 8x8 grey block (DC 0, end-of-block), with a COM segment naming the sample."""

    def seg(marker: int, payload: bytes) -> bytes:
        return struct.pack(">BBH", 0xFF, marker, len(payload) + 2) + payload

    com = comment.encode("ascii")
    dqt = bytes([0x00]) + bytes([1] * 64)  # table 0, 8-bit, all ones
    sof = struct.pack(">BHHB", 8, 8, 8, 1) + bytes([1, 0x11, 0])  # 8x8, 1 component, table 0
    # One Huffman code of length 1 ("0") per table: DC category 0, AC end-of-block.
    dht_dc = bytes([0x00]) + bytes([1] + [0] * 15) + bytes([0x00])
    dht_ac = bytes([0x10]) + bytes([1] + [0] * 15) + bytes([0x00])
    sos = bytes([1, 1, 0x00, 0, 63, 0])
    scan = bytes([0b00111111])  # DC "0" + EOB "0", padded with ones
    return (
        b"\xff\xd8"
        + seg(0xFE, com)
        + seg(0xDB, dqt)
        + seg(0xC0, sof)
        + seg(0xC4, dht_dc)
        + seg(0xC4, dht_ac)
        + seg(0xDA, sos)
        + scan
        + b"\xff\xd9"
    )


def write(path: Path, data) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(data, str):
        data = data.encode("utf-8")
    path.write_bytes(data)


def lines(*ls: str) -> str:
    return "\n".join(ls) + "\n"


def expected(name: str, source: str, path: str, tz, date_order, conversations, events, packed, missing, findings):
    write(
        EXPECTED / f"{name}.json",
        json.dumps(
            {
                "source": source,
                "path": path,
                "options": {"timezone": tz, "date_order": date_order},
                "conversations": conversations,
                "events": events,
                "attachments_packed": packed,
                "attachments_missing": missing,
                "findings": sorted(set(findings)),
            },
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
    )


def whatsapp_android_en() -> None:
    folder = EVIDENCE / "whatsapp-android-en"
    num = f"{LRE}+33 6 00 00 00 01{PDF}"
    chat = lines(
        "02/03/2026, 09:58 - Messages and calls are end-to-end encrypted. No one outside of this chat, "
        "not even WhatsApp, can read or listen to them. Tap to learn more.",
        '02/03/2026, 09:59 - Ana Ruiz created group "Proyecto Faro"',
        f"02/03/2026, 09:59 - Ana Ruiz added {num}",
        "02/03/2026, 10:00 - Ana Ruiz: Hola! Plan for the 14th:",
        "1. Kick-off at 10:00",
        "2. Review: budget & timeline",
        "02/03/2026, 10:01 - Ana Ruiz: IMG-20260302-WA0001.jpg (file attached)",
        f"02/03/2026, 10:02 - {num}: Got it \U0001f44d thanks",
        "02/03/2026, 10:03 - Bruno Diaz: IMG-20260302-WA0002.jpg (file attached)",
        "Photo from the site",
        "13/03/2026, 18:45 - Bruno Diaz: <Media omitted>",
        "13/03/2026, 18:46 - Ana Ruiz: Note: the deadline is 20/03 at 17:00",
        "14/03/2026, 08:00 - Bruno Diaz left",
    )
    write(folder / "WhatsApp Chat with Ana Ruiz.txt", chat.replace("\n", "\r\n"))  # Android writes CRLF on some versions
    write(folder / "IMG-20260302-WA0001.jpg", tiny_jpeg("rv-rsmf synthetic sample IMG-20260302-WA0001"))
    # 10 events: disclaimer, created (history), added (join), 6 messages, left (leave).
    # WA0001 present -> packed; WA0002 absent -> missing; <Media omitted> -> missing.
    expected(
        "whatsapp-android-en",
        "whatsapp",
        "whatsapp-android-en/WhatsApp Chat with Ana Ruiz.txt",
        "Europe/Paris",
        None,
        1,
        10,
        1,
        2,
        [
            "DATE_ORDER_INFERRED",
            "MEDIA_OMITTED_IN_EXPORT",
            "ATTACHMENT_REFERENCED_NOT_FOUND",
            "PARTICIPANT_IDENTIFIED_BY_NUMBER_ONLY",
        ],
    )


def whatsapp_ios_es() -> None:
    pm = f"{NNBSP}p.{NBSP}m."
    am = f"{NNBSP}a.{NBSP}m."
    photo = "00000003-PHOTO-2026-03-04-21-15-02.jpg"
    chat = lines(
        f"[4/3/26, 9:10:00{pm}] Lucía Gómez: {LRM}Los mensajes y las llamadas están cifrados de extremo a extremo. "
        "Nadie fuera de este chat, ni siquiera WhatsApp, puede leerlos ni escucharlos.",
        f"[4/3/26, 9:12:30{pm}] Lucía Gómez: Hola, ¿recibiste el contrato?",
        f"[4/3/26, 9:14:10{pm}] Tú: Sí, te mando la foto",
        f"{LRM}[4/3/26, 9:15:02{pm}] Tú: {LRM}<adjunto: {photo}>",
        f"[15/3/26, 12:05:44{am}] Lucía Gómez: Perfecto, gracias.",
        "Hablamos el lunes.",
    )
    out = EVIDENCE / "whatsapp-ios-es.zip"
    out.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_STORED) as z:
        for name, data, when in [
            ("_chat.txt", chat.encode("utf-8"), (2026, 3, 16, 9, 0, 0)),
            (photo, tiny_jpeg("rv-rsmf synthetic sample " + photo[:-4]), (2026, 3, 4, 21, 15, 2)),
        ]:
            info = zipfile.ZipInfo(name, date_time=when)
            info.external_attr = 0o644 << 16
            z.writestr(info, data)
    # 5 events: disclaimer (attributed to the chat name on iOS) + 4 messages; the photo is in the zip.
    expected("whatsapp-ios-es", "whatsapp", "whatsapp-ios-es.zip", "America/Bogota", None, 1, 5, 1, 0, ["DATE_ORDER_INFERRED"])


def whatsapp_ambiguous() -> None:
    chat = lines(
        "03/04/26, 9:05 AM - Carla Méndez: Buenos días",
        "03/04/26, 9:06 AM - Diego Torres: Hi Carla, see you on 05/06?",
        "05/04/26, 1:30 PM - Carla Méndez: Yes: 10 AM at the office",
        "05/04/26, 12:00 AM - Diego Torres: Midnight test",
    )
    write(EVIDENCE / "whatsapp-ambiguous.txt", chat)
    expected("whatsapp-ambiguous", "whatsapp", "whatsapp-ambiguous.txt", "Europe/Madrid", "dmy", 1, 4, 0, 0, ["DATE_ORDER_CHOSEN"])


def whatsapp_dst() -> None:
    chat = lines(
        "Synthetic sample for rv-rsmf tests - invented people only",
        "28.03.26, 23:59 - Eva Klein: Clocks go forward tonight",
        "29.03.26, 02:30 - Eva Klein: This local time does not exist in Paris",
        "29.03.26, 03:30 - Felix Braun: Summer time now",
        "25.10.26, 02:30 - Felix Braun: This local time happens twice",
        "25.10.26, 02:45 - Eva Klein: Me too",
    )
    write(EVIDENCE / "whatsapp-dst.txt", chat)
    # 02:30 on 29 March does not exist; 02:30 and 02:45 on 25 October happen twice -> 3 DST problems.
    expected(
        "whatsapp-dst",
        "whatsapp",
        "whatsapp-dst.txt",
        "Europe/Paris",
        None,
        1,
        5,
        0,
        0,
        ["DATE_ORDER_INFERRED", "DST_AMBIGUOUS_OR_MISSING_TIME", "UNPARSED_LINES"],
    )


PARIS_WINTER = timezone(timedelta(hours=1))


def tg_time(local: str) -> dict:
    """Telegram Desktop writes the local time and the epoch seconds as a string."""
    t = datetime.strptime(local, "%Y-%m-%dT%H:%M:%S").replace(tzinfo=PARIS_WINTER)
    return {"date": local, "date_unixtime": str(int(t.timestamp()))}


def telegram() -> None:
    folder = EVIDENCE / "telegram"
    gab = {"from": "Gabriel Lenoir", "from_id": "user1000000001"}
    hugo = {"from": "Hugo Martin", "from_id": "user1000000002"}
    iris = {"from": "Iris Bonnet", "from_id": "user1000000003"}
    not_included = "(File not included. Change data exporting settings to download.)"

    def msg(mid, local, who, text, **extra):
        m = {"id": mid, "type": "message", **tg_time(local), **who, "text": text}
        if isinstance(text, str):
            m["text_entities"] = [{"type": "plain", "text": text}] if text else []
        else:
            m["text_entities"] = [p if isinstance(p, dict) else {"type": "plain", "text": p} for p in text]
        m.update(extra)
        return m

    def service(mid, local, actor, action, **extra):
        return {
            "id": mid,
            "type": "service",
            **tg_time(local),
            "actor": actor["from"],
            "actor_id": actor["from_id"],
            "action": action,
            **extra,
            "text": "",
            "text_entities": [],
        }

    edited = tg_time("2026-03-02T10:12:00")
    personal = {
        "name": "Hugo Martin",
        "type": "personal_chat",
        "id": 2000000001,
        "messages": [
            msg(101, "2026-03-02T10:00:00", gab, "Voici la photo du tableau",
                photo="photos/photo_1@02-03-2026_10-00-00.jpg", width=8, height=8),
            msg(102, "2026-03-02T10:05:00", hugo,
                ["Merci ! Voir ", {"type": "text_link", "text": "le dossier", "href": "https://example.org/dossier"},
                 " et ", {"type": "bold", "text": "vite"}, "."],
                reply_to_message_id=101,
                reactions=[{"type": "emoji", "count": 1, "emoji": "\U0001f44d",
                            "recent": [{**gab, "date": "2026-03-02T10:06:00"}]}]),
            msg(103, "2026-03-02T10:10:00", gab, "Rendez-vous à 15 h (corrigé)",
                edited=edited["date"], edited_unixtime=edited["date_unixtime"]),
            msg(104, "2026-03-02T11:00:00", hugo, "Le contrat",
                file=not_included, file_name="contrat.pdf", mime_type="application/pdf"),
            service(105, "2026-03-02T12:00:00", hugo, "phone_call", duration_seconds=125, discard_reason="hangup"),
            msg(106, "2026-03-02T12:30:00", gab, "",
                contact_information={"first_name": "Julie", "last_name": "Petit", "phone_number": "+33 6 00 00 00 03"}),
            msg(107, "2026-03-02T12:40:00", hugo, "",
                file="voice_messages/audio_1@02-03-2026_12-40-00.ogg", file_name="audio_1@02-03-2026_12-40-00.ogg",
                media_type="voice_message", mime_type="audio/ogg", duration_seconds=4),
        ],
    }
    group = {
        "name": "Équipe Atlas",
        "type": "private_supergroup",
        "id": 3000000001,
        "messages": [
            service(201, "2026-03-03T09:00:00", gab, "create_group", title="Équipe Atlas", members=["Gabriel Lenoir"]),
            service(202, "2026-03-03T09:01:00", gab, "invite_members", members=["Iris Bonnet"]),
            msg(203, "2026-03-03T09:05:00", iris,
                ["Bonjour à tous, le lien : ", {"type": "link", "text": "https://example.org/atlas"}],
                reactions=[
                    {"type": "emoji", "count": 2, "emoji": "❤",
                     "recent": [{**gab, "date": "2026-03-03T09:06:00"}, {**hugo, "date": "2026-03-03T09:07:00"}]},
                    {"type": "custom_emoji", "count": 1, "document_id": "stickers/custom_emoji_1.webp",
                     "recent": [{**iris, "date": "2026-03-03T09:08:00"}]},
                ]),
            msg(204, "2026-03-03T09:10:00", gab, "Plan", photo="photos/photo_2@03-03-2026_09-10-00.jpg", width=8, height=8),
            msg(205, "2026-03-03T09:15:00", iris, "", location_information={"latitude": 48.8566, "longitude": 2.3522}),
            msg(206, "2026-03-03T09:20:00", gab, "",
                poll={"question": "Réunion jeudi ?", "closed": False, "total_voters": 2,
                      "answers": [{"text": "Oui", "voters": 2, "chosen": True},
                                  {"text": "Non", "voters": 0, "chosen": False}]}),
            service(207, "2026-03-03T09:30:00", iris, "pin_message", message_id=203),
            service(208, "2026-03-03T18:00:00", iris, "remove_members", members=["Iris Bonnet"]),
        ],
    }
    export = {
        "about": "Here is the data you requested. Synthetic sample for rv-rsmf tests: every person is invented.",
        "personal_information": {
            "user_id": 1000000001,
            "first_name": "Gabriel",
            "last_name": "Lenoir",
            "phone_number": "+33 6 00 00 00 02",
            "bio": "",
        },
        "chats": {"about": "This page lists all chats from this export.", "list": [personal, group]},
    }
    write(folder / "result.json", json.dumps(export, ensure_ascii=False, indent=1) + "\n")
    write(folder / "photos" / "photo_1@02-03-2026_10-00-00.jpg", tiny_jpeg("rv-rsmf synthetic sample photo_1"))
    write(folder / "voice_messages" / "audio_1@02-03-2026_12-40-00.ogg", b"OggS\x00rv-rsmf synthetic sample voice message")
    # photos/photo_2@... is deliberately NOT written: a referenced path that does not exist.
    # 15 events in 2 chats; packed: photo_1, audio_1; missing: contrat.pdf (not included), photo_2 (absent).
    expected(
        "telegram",
        "telegram",
        "telegram/result.json",
        None,
        None,
        2,
        15,
        2,
        2,
        ["TELEGRAM_FILE_NOT_INCLUDED", "ATTACHMENT_REFERENCED_NOT_FOUND", "EDIT_HISTORY_NOT_IN_EXPORT"],
    )


def main() -> None:
    whatsapp_android_en()
    whatsapp_ios_es()
    whatsapp_ambiguous()
    whatsapp_dst()
    telegram()
    print(f"samples written to {EVIDENCE.relative_to(REPO)} and {EXPECTED.relative_to(REPO)}")


if __name__ == "__main__":
    main()
