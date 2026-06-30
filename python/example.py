from ic705bridge import IC705Bridge, split_frames

RADIO = "A4"  # adresse CI-V de l'IC-705
CTRL = "E0"   # adresse du contrôleur (PC)

MODES = {
    0x00: "LSB", 0x01: "USB", 0x02: "AM", 0x03: "CW", 0x04: "RTTY",
    0x05: "FM", 0x07: "CW-R", 0x08: "RTTY-R", 0x17: "DV",
}


def reply_payload(response_hex, cmd):
    for frame in split_frames(response_hex):
        b = bytes.fromhex(frame.replace(" ", ""))
        if len(b) >= 6 and b[2] == 0xE0 and b[3] == 0xA4 and b[4] == cmd:
            return b[5:-1]
    return None


def decode_bcd_freq(payload):
    hz, mult = 0, 1
    for byte in payload:
        hz += (byte & 0x0F) * mult
        mult *= 10
        hz += (byte >> 4) * mult
        mult *= 10
    return hz


def decode_bcd_int(payload):
    value = 0
    for byte in payload:
        value = value * 100 + (byte >> 4) * 10 + (byte & 0x0F)
    return value


def main():
    rig = IC705Bridge()

    # === STATUS ===
    print("=== STATUS ===")
    st = rig.status()
    print(f"  état       : {st.get('state')}")
    print(f"  hôte       : {st.get('host')}")
    print(f"  API        : {st.get('api_url')} (running={st.get('api_running')})")

    if not rig.is_ready():
        print("\n⚠ L'IC-705 n'est pas connecté. Connecte-toi dans l'application IC705-Bridge d'abord.")
        return 1

    # === RX (lectures) ===
    print("\n=== RX (lectures) ===")

    rep = rig.send_civ(f"FE FE {RADIO} {CTRL} 03 FD")          # fréquence
    payload = reply_payload(rep["response"], 0x03)
    if payload:
        print(f"  fréquence  : {decode_bcd_freq(payload) / 1_000_000:.6f} MHz")

    rep = rig.send_civ(f"FE FE {RADIO} {CTRL} 04 FD")          # mode
    payload = reply_payload(rep["response"], 0x04)
    if payload:
        print(f"  mode       : {MODES.get(payload[0], f'0x{payload[0]:02X}')}")

    rep = rig.send_civ(f"FE FE {RADIO} {CTRL} 15 02 FD")       # S-mètre
    payload = reply_payload(rep["response"], 0x15)
    if payload:
        print(f"  S-mètre    : {decode_bcd_int(payload[1:])} / 255")

    rep = rig.send_civ(f"FE FE {RADIO} {CTRL} 1C 00 FD")       # statut PTT
    payload = reply_payload(rep["response"], 0x1C)
    if payload:
        print(f"  PTT        : {'TX' if payload[1] else 'RX'}")

    # === TX (écritures sûres : on règle sans émettre) ===
    print("\n=== TX (écritures) ===")

    rep = rig.send_civ(f"FE FE {RADIO} {CTRL} 06 01 FD")       # mode USB
    print(f"  set mode USB : {'OK' if reply_payload(rep['response'], 0xFB) is not None else rep['response']}")

    rep = rig.send_civ(f"FE FE {RADIO} {CTRL} 05 00 00 50 45 01 FD")  # 145.500 MHz
    print(f"  set 145.500  : {'OK' if reply_payload(rep['response'], 0xFB) is not None else rep['response']}")

    # PTT (émission RF !) — n'activer qu'avec antenne/charge fictive.
    demo_ptt = False
    if demo_ptt:
        rig.send_civ(f"FE FE {RADIO} {CTRL} 1C 00 01 FD")     # PTT ON
        rig.send_civ(f"FE FE {RADIO} {CTRL} 1C 00 00 FD")     # PTT OFF

    # === STREAM (lecture continue des trames CI-V) ===
    # rig.stream_civ() est un générateur bloquant : décommenter pour écouter.
    # print("\n=== STREAM (Ctrl-C pour arrêter) ===")
    # for frame in rig.stream_civ():
    #     print("  RX:", frame)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
