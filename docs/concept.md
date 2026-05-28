# Agora Coin — Technische Spezifikation v1.0

**Dokumenttyp:** Implementierungsvorgabe für Entwicklungsteam / Code-Generierungssystem
**Zielsprache:** Rust (Backend), Leptos oder React (Frontend)
**Status:** Prototyp-Spezifikation, Phase 1 (Single-Validator → Multi-Validator)

---

## Teil A — Systemüberblick

### A.1 Zweck

Agora Coin (AGC) ist ein semi-dezentrales Token-System mit eingebautem Umverteilungsmechanismus. Die Wallet-Erstellung ist permissioned (über autorisierte Operatoren), Transaktionen sind permissionless innerhalb des Nutzerkreises. Periodische Iterationen reduzieren Vermögensungleichheit durch logarithmische Strafen und aktivitätsgewichtete Redistribution.

### A.2 Designprinzipien

1. **Pseudonymität**: Wallet-ID = Hash eines öffentlichen Schlüssels; keine personenbezogenen Daten on-chain.
2. **Permissioned Issuance**: Nur Operatoren mit gültigem Schlüssel können Wallets erstellen und AGC minten (Charge).
3. **Permissionless Transfer**: Jeder Wallet-Inhaber kann signierte Transaktionen senden.
4. **Verteilte Validierung**: N Validator-Nodes führen PoH-Stream und Konsens-Logik.
5. **Ökonomische Selbstregulation**: Penalty-Burn-Mechanismus reagiert dynamisch auf Inflation.
6. **Vollständige Auditierbarkeit**: Alle Operationen (Mint, Transfer, Iteration) sind on-chain und replay-bar.

### A.3 Komponentendiagramm

```
[Agora-Plattform (extern)] — signiert mit Operator-Key —┐
                                                         │
[Agora-Coin REST Gateway] ◄────────────────────────────┘
        │
        ├── /wallet      (Operator-Sig erforderlich)
        ├── /charge      (Operator-Sig erforderlich)
        ├── /tx          (User-Sig erforderlich)
        ├── /query/*     (offen)
        └── /admin/*     (Master-Sig erforderlich)
        │
        ▼
[Validator-Netzwerk: 1..N Nodes]
        │
        ├── PoH-Stream (sequentielle Hash-Kette)
        ├── Mempool (pending TXs)
        ├── Consensus (Round-Robin Leader)
        └── State-Machine (Wallet-Balances)
        │
        ▼
[Persistenz-Layer: sled + JSON-Snapshots]
        │
        ▼
[Iteration-Engine (periodisch, alle ~30 Tage)]
```

---

## Teil B — Datenmodell

### B.1 Primitive Typen

| Typ | Repräsentation | Größe | Beschreibung |
|---|---|---|---|
| `WalletId` | `[u8; 32]` | 32 B | `BLAKE3(public_key)` |
| `OperatorId` | `[u8; 32]` | 32 B | `BLAKE3(operator_pubkey)` |
| `ValidatorId` | `[u8; 32]` | 32 B | `BLAKE3(validator_pubkey)` |
| `PublicKey` | `[u8; 32]` | 32 B | Ed25519 |
| `Signature` | `[u8; 64]` | 64 B | Ed25519 |
| `Hash` | `[u8; 32]` | 32 B | BLAKE3-Output |
| `MicroAgc` | `u64` | 8 B | 1 AGC = 1.000.000 µAGC |
| `Tick` | `u64` | 8 B | PoH-Tick-Index |
| `IterationId` | `u64` | 8 B | Monoton steigend, beginnt bei 0 |
| `Nonce` | `u64` | 8 B | Per-Wallet, monoton steigend |
| `Timestamp` | `i64` | 8 B | Unix Epoch in Millisekunden |

### B.2 Operator-Registry

```
Operator {
    id:              OperatorId
    pubkey:          PublicKey
    role:            { Genesis, Standard }
    authorized_at:   Tick
    authorized_by:   OperatorId          // self bei Genesis
    revoked_at:      Option<Tick>
    metadata:        String              // optional, z.B. "Agora Platform Primary"
}
```

**Invarianten:**
- Genau ein Operator hat `role = Genesis`.
- Genesis-Operator existiert ab Tick 0.
- Standard-Operatoren werden durch Operator-Mehrheit (oder anfangs nur durch Genesis) hinzugefügt.
- Im Prototyp v1.0: **N_operators = 1**, ausschließlich Genesis aktiv.

### B.3 Wallet

```
Wallet {
    id:                       WalletId
    pubkey:                   PublicKey
    balance:                  MicroAgc
    nonce:                    Nonce             // nächster erwarteter Nonce
    created_at_tick:          Tick
    created_at_iteration:     IterationId
    created_by_operator:      OperatorId
    
    // Aktivitäts-Tracking (per Iteration zurückgesetzt)
    iteration_tx_count:       u32
    iteration_counterparties: Set<WalletId>
    last_active_iteration:    IterationId
    
    // Persistierter Aktivitäts-Score (EMA über Iterationen)
    activity_score:           f64               // [0.0, 1.0]
}
```

**Invarianten:**
- `balance ≥ 0` (u64 garantiert).
- `nonce` strikt monoton steigend pro Wallet.
- Initial nach Erstellung: `balance = initial_seed`, `nonce = 0`, `activity_score = 1.0`.

### B.4 Transaktionstypen

Alle Transaktionen werden vor Aufnahme in den PoH-Stream signiert und verifiziert.

```
Transaction = OneOf {
    Mint, Transfer, Burn, IterationCommit, OperatorChange
}
```

#### B.4.1 Mint (Charge)

```
Mint {
    tx_id:          Hash                  // BLAKE3 über alle Felder außer Signatur
    to:             WalletId
    amount:         MicroAgc
    eur_amount:     u64                   // gezahlte EUR-Cent (audit)
    operator_id:    OperatorId
    nonce:          u64                   // Operator-Nonce
    timestamp:      Timestamp
    operator_sig:   Signature             // Sig durch Operator-Key
}
```

#### B.4.2 Transfer

```
Transfer {
    tx_id:          Hash
    from:           WalletId
    to:             WalletId
    amount:         MicroAgc
    nonce:          u64                   // Wallet-Nonce
    timestamp:      Timestamp
    sender_pubkey:  PublicKey             // muss zu `from` hashen
    user_sig:       Signature
}
```

**Validierungsregeln:**
- `BLAKE3(sender_pubkey) == from`
- `verify(user_sig, payload, sender_pubkey) == true`
- `wallet[from].nonce == nonce`
- `wallet[from].balance ≥ amount`
- `from ≠ to`
- `amount > 0`

#### B.4.3 Burn

```
Burn {
    tx_id:          Hash
    from:           WalletId
    amount:         MicroAgc
    nonce:          u64
    timestamp:      Timestamp
    sender_pubkey:  PublicKey
    user_sig:       Signature
}
```

#### B.4.4 IterationCommit (system-generiert)

```
IterationCommit {
    tx_id:               Hash
    iteration_id:        IterationId
    triggered_at_tick:   Tick
    
    snapshot_supply:     MicroAgc          // Gesamtmenge vor Iteration
    snapshot_n_wallets:  u64
    snapshot_gini:       f64
    
    penalties:           Vec<(WalletId, MicroAgc)>
    rewards:             Vec<(WalletId, MicroAgc)>
    burned:              MicroAgc
    
    new_wallets:         Vec<WalletId>     // in dieser Iteration erstellt
    
    post_supply:         MicroAgc          // Gesamtmenge nach Iteration
    validator_sigs:      Vec<(ValidatorId, Signature)>
}
```

#### B.4.5 OperatorChange

```
OperatorChange {
    tx_id:           Hash
    action:          { Add, Revoke }
    target_pubkey:   PublicKey
    timestamp:       Timestamp
    authorizing_op:  OperatorId
    operator_sig:    Signature
}
```

Im Prototyp v1.0 deaktiviert (nur Genesis-Operator).

### B.5 Proof-of-History-Eintrag

```
PohEntry {
    tick:              Tick
    prev_hash:         Hash
    hash:              Hash                  // siehe Formel C.1
    tx_root:           Hash                  // Merkle-Root der enthaltenen TXs
    tx_ids:            Vec<Hash>
    leader_id:         ValidatorId
    leader_sig:        Signature             // Sig über (tick, hash, tx_root)
    wall_clock:        Timestamp             // wall-clock zum Vergleich
}
```

### B.6 Validator

```
Validator {
    id:               ValidatorId
    pubkey:           PublicKey
    endpoint:         String                // host:port
    is_genesis:       bool
    registered_at:    Tick
    active:           bool
    last_seen_tick:   Tick
}
```

### B.7 Globaler System-State

```
SystemState {
    current_tick:        Tick
    current_iteration:   IterationId
    iteration_started_at: Tick
    
    total_supply:        MicroAgc
    n_wallets:           u64
    n_active_wallets:    u64
    
    last_poh_hash:       Hash
    
    parameters:          SystemParameters    // siehe Teil F
}
```

---

## Teil C — Kryptographische Konstruktionen

### C.1 PoH-Hash-Funktion

```
PohEntry.hash = BLAKE3(
    prev_hash  ||
    tick_LE_bytes  ||
    tx_root  ||
    leader_id
)
```

Sequentielle Berechnung: Tick N kann nicht vor Tick N-1 berechnet werden. Dies etabliert eine kryptographisch verifizierbare Zeitordnung.

**Tick-Rate:** `poh_tick_ms = 400` (default). Validator erzeugt 2.5 Ticks/Sekunde.

### C.2 Wallet-ID-Ableitung

```
WalletId = BLAKE3(public_key_bytes)
```

Kollisionsresistenz: 2^128 (BLAKE3 256-bit Output).

### C.3 Transaktions-ID

```
tx_id = BLAKE3( canonical_serialize(tx_payload_without_signature) )
```

Kanonische Serialisierung: alle Felder in fester Reihenfolge, little-endian, length-prefixed bei variabler Größe. Empfehlung: `bincode` mit explizit definiertem Layout oder `borsh`.

### C.4 Signaturen

```
signature = Ed25519_sign(secret_key, tx_id)
```

Verifikation:
```
Ed25519_verify(public_key, tx_id, signature) == true
```

### C.5 Merkle-Root für TX-Bündel

Binärbaum, BLAKE3, doppelte Hashes bei ungeraden Blattzahlen:

```
leaf_i  = BLAKE3(0x00 || tx_id_i)
node    = BLAKE3(0x01 || left || right)
root    = wiederholt bis ein Knoten übrig
```

Bei leerer TX-Liste: `tx_root = [0u8; 32]`.

---

## Teil D — Ökonomisches Modell (mathematisch)

### D.1 Bezeichnungen

| Symbol | Bedeutung |
|---|---|
| $b_i$ | Saldo von Wallet $i$ (in µAGC) |
| $N$ | Anzahl Wallets am Iterations-Beginn |
| $N_a$ | Anzahl aktiver Wallets in Iteration |
| $M$ | Gesamtmenge (Supply) |
| $g$ | Wachstumsfaktor neuer Wallets |
| $s$ | Initial-Seed pro neuem Wallet |
| $r$ | Penalty-Rate |
| $\alpha_i$ | Aktivitäts-Multiplikator von Wallet $i$ |
| $\beta$ | Burn-Anteil |
| $I$ | Inflations-Rate der letzten Iteration |
| $I^*$ | Ziel-Inflations-Rate |
| $P$ | Gesamt-Penalty-Pool |
| $B$ | Gebrannter Betrag |
| $R$ | Redistribuierter Betrag |

### D.2 Penalty-Berechnung (logarithmisch, alle Wallets)

Pro Wallet $i$:

$$\text{penalty}_i = \left\lfloor r \cdot b_i \cdot \frac{\ln(1 + b_i / b_{\text{ref}})}{\ln(1 + b_{\max} / b_{\text{ref}})} \right\rfloor$$

mit:
- $b_{\text{ref}} = s$ (Referenz = Initial-Seed), normalisiert die Log-Skala
- $b_{\max}$ = aktuell größter Saldo im System

**Eigenschaften:**
- Wallet mit $b_i = 0$: $\text{penalty}_i = 0$ (Log(1) = 0).
- Wallet mit $b_i = s$: kleinste positive Strafe.
- Wallet mit $b_i = b_{\max}$: $\text{penalty}_i = r \cdot b_{\max}$.
- Monoton steigend in $b_i$.
- Konkav: zusätzliche Einheiten werden zunehmend schwächer bestraft (Log-Wachstum), aber relativ zum Gesamtsaldo wird der Reiche dennoch stärker belastet.

**Alternative (einfacher, ebenfalls logarithmisch):**

$$\text{penalty}_i = \lfloor r \cdot \ln(1 + b_i) \cdot k \rfloor$$

mit Skalierungskonstante $k$ so gewählt, dass $\sum_i \text{penalty}_i \approx \tau \cdot M$ für Ziel-Steueranteil $\tau$.

**Im Prototyp:** Erste Variante verwenden. Skalierungskonstante automatisch so kalibrieren, dass der Penalty-Pool eine konfigurierbare Ziel-Quote der Gesamt-Supply trifft.

### D.3 Penalty-Pool

$$P = \sum_{i=1}^{N} \text{penalty}_i$$

### D.4 Adaptive Burn-Rate

Inflation der vorhergehenden Iteration:

$$I = \frac{M_{\text{ende, prev}} - M_{\text{anfang, prev}}}{M_{\text{anfang, prev}}}$$

Burn-Anteil dynamisch:

$$\beta = \text{clip}\left(\beta_0 + k_\beta \cdot (I - I^*),\ \beta_{\min},\ \beta_{\max}\right)$$

mit:
- $\beta_0$ = Basis-Burn-Rate (z. B. 0.10)
- $k_\beta$ = Sensitivität (z. B. 0.5)
- $I^*$ = Ziel-Inflation (z. B. 0.02 = 2 % pro Iteration)
- $\beta_{\min} = 0.0$, $\beta_{\max} = 0.9$

**Interpretation:** Bei hoher Inflation wird mehr gebrannt, bei niedriger weniger. Dies erzeugt einen Regelkreis.

### D.5 Burn und Redistribution

$$B = \lfloor \beta \cdot P \rfloor$$
$$R = P - B$$

### D.6 Aktivitäts-Score

Pro Wallet, am Iterationsende:

$$a_i^{\text{raw}} = \begin{cases}
1 & \text{falls } \text{tx\_count}_i \geq t_{\min} \land |\text{counterparties}_i| \geq c_{\min} \\
0 & \text{sonst}
\end{cases}$$

Exponentiell geglätteter Score:

$$\text{activity\_score}_i^{(t+1)} = \lambda \cdot a_i^{\text{raw}} + (1-\lambda) \cdot \text{activity\_score}_i^{(t)}$$

mit $\lambda \in [0,1]$ (Default $\lambda = 0.5$).

Multiplikator für Redistribution:

$$\alpha_i = \alpha_{\min} + (\alpha_{\max} - \alpha_{\min}) \cdot \text{activity\_score}_i$$

Default: $\alpha_{\min} = 0.5$, $\alpha_{\max} = 1.5$.

### D.7 Redistribution

Anteil von Wallet $i$ am Redistribution-Pool $R$:

$$\text{share}_i = \frac{\alpha_i}{\sum_{j=1}^{N} \alpha_j}$$

$$\text{reward}_i = \lfloor R \cdot \text{share}_i \rfloor$$

**Rundungs-Residual:** Differenz $R - \sum_i \text{reward}_i$ wird dem Genesis-Wallet zugeschrieben oder zusätzlich gebrannt (zu konfigurieren).

**Variante mit umgekehrter Saldo-Gewichtung (optional):**

$$\text{share}_i = \frac{\alpha_i \cdot (1 + b_i)^{-\gamma}}{\sum_j \alpha_j \cdot (1 + b_j)^{-\gamma}}$$

mit $\gamma \in [0, 1]$ (Default $\gamma = 0$ = nur aktivitätsbasiert; $\gamma = 1$ = stark umgekehrt-proportional).

### D.8 Saldo-Update

$$b_i^{\text{neu}} = b_i - \text{penalty}_i + \text{reward}_i$$

### D.9 Wallet-Wachstum

Anzahl neuer Wallets in Iteration $t+1$:

$$N_{\text{new}}^{(t+1)} = \min\left( \lfloor g \cdot N^{(t)} \rfloor,\ N_{\max}^{\text{cap}} \right)$$

Jedes neue Wallet erhält $s$ µAGC aus einem **Faucet-Pool**.

### D.10 Faucet-Pool

Quellen:
1. Ein Teil des Penalty-Pools (vor Burn/Redistribution abgezweigt), Anteil $\phi$.
2. Bei Unterdeckung: Mint durch Genesis-Operator (loggt als `Mint` mit speziellem `eur_amount = 0`, kategorisiert als Faucet-Mint).

Faucet-Bedarf pro Iteration:

$$F = N_{\text{new}} \cdot s$$

Aus Penalty:

$$F_{\text{from\_penalty}} = \min(F,\ \phi \cdot P)$$

Restbedarf:

$$F_{\text{from\_mint}} = F - F_{\text{from\_penalty}}$$

### D.11 Supply-Bilanz pro Iteration

$$M^{(t+1)} = M^{(t)} - B + F_{\text{from\_mint}} + \text{Charge-Mints}^{(t \to t+1)}$$

Validierung in `IterationCommit`:

$$M^{(t+1)} \stackrel{!}{=} \sum_i b_i^{\text{neu}} + \sum_{j \in \text{new}} s$$

### D.12 Gini-Koeffizient (Metrik)

$$G = \frac{\sum_{i=1}^{N} \sum_{j=1}^{N} |b_i - b_j|}{2 N \sum_{i=1}^{N} b_i}$$

Berechnung in O(N log N) durch Sortierung:

$$G = \frac{2 \sum_{i=1}^{N} i \cdot b_{(i)}}{N \sum_{i=1}^{N} b_{(i)}} - \frac{N+1}{N}$$

mit $b_{(i)}$ = i-tkleinster Saldo.

### D.13 Stabilitäts-Analyse (informell)

Gleichgewichts-Bedingung (Supply stationär):

$$B \stackrel{!}{=} F_{\text{from\_mint}} + \text{Charge-Mints}$$

D. h. der Burn-Mechanismus muss Wallet-Wachstum und Charge-Mints kompensieren, um Hyperinflation zu vermeiden. Der adaptive Burn ($\beta$) regelt dies automatisch — vorausgesetzt $P$ ist groß genug.

**Erforderliche Validierung vor Produktion**: Numerische Simulation über mindestens 24 Iterationen mit verschiedenen Anfangsbedingungen, um Konvergenz zu prüfen.

---

## Teil E — Konsens & PoH-Mechanik

### E.1 Konsens-Modell (Prototyp v1.0)

- **N Validator-Nodes**, konfigurierbar (default: 1 = Genesis).
- **Round-Robin Leader-Rotation**: Pro Slot von `S = 64` Ticks wechselt der Leader. Reihenfolge: deterministisch nach `ValidatorId` (sortiert lexikographisch).
- **Crash-Tolerant, nicht Byzantine-Tolerant**: Annahme: Validators sind ehrlich. BFT in v2.0.
- **Bei N = 1**: kein Leader-Wechsel, Genesis ist permanent Leader.

### E.2 Leader-Pflichten

Pro Tick im eigenen Slot:
1. TXs aus Mempool ziehen (max. `max_txs_per_tick`, default 256).
2. Jede TX kryptographisch und semantisch validieren.
3. Merkle-Root berechnen.
4. PoH-Hash berechnen (siehe C.1).
5. PoH-Entry signieren.
6. Entry an alle anderen Validators broadcasten.
7. Lokal persistieren.

### E.3 Follower-Pflichten

Pro empfangenem PoH-Entry:
1. Signatur des Leaders verifizieren.
2. Hash-Kette verifizieren (`prev_hash` muss zum letzten lokalen Hash passen).
3. TX-Root nachrechnen.
4. Jede TX im Entry erneut validieren.
5. State-Machine anwenden.
6. Lokal persistieren.

### E.4 Slot-Abschluss

Am Ende jedes Slots: Snapshot des State-Hashes broadcasten. Andere Validators vergleichen. Bei Divergenz: Logging + Halt (kein automatisches Recovery im Prototyp).

### E.5 Tick- vs. Iteration-Beziehung

| Größe | Default | Berechnung |
|---|---|---|
| Tick-Dauer | 400 ms | `poh_tick_ms` |
| Ticks pro Sekunde | 2.5 | `1000 / 400` |
| Slot-Dauer | 25.6 s | `64 ticks × 400 ms` |
| Iterationen pro Jahr | 12 | Konfigurierbar |
| Iterations-Intervall | ~30.4 Tage | `365 / 12` |
| Ticks pro Iteration | ~6.566.400 | `30.4 × 86400 × 2.5` |

Im Simulator: Tick-Dauer auf 1 ms oder schneller setzen → 1 Jahr in Minuten simulierbar.

---

## Teil F — System-Parameter

Alle in einer zentralen TOML-Konfig, zur Laufzeit über Admin-API änderbar (mit Operator-Signatur). Änderungen werden als spezielle Transaktionen geloggt.

```
[economy]
initial_seed_agc              = 10            # AGC pro neuem Wallet
penalty_rate                  = 0.05          # r in D.2
target_penalty_share_of_supply = 0.03         # τ, für Skalierungs-Auto-Kalibrierung
burn_base                     = 0.10          # β₀
burn_sensitivity              = 0.5           # k_β
burn_min                      = 0.0
burn_max                      = 0.9
target_inflation_per_iter     = 0.02          # I*
faucet_share_of_penalty       = 0.20          # φ
redistribution_active_min     = 0.5           # α_min
redistribution_active_max     = 1.5           # α_max
activity_ema_lambda           = 0.5
activity_min_tx_count         = 1
activity_min_counterparties   = 1
inverse_balance_weight        = 0.0           # γ in D.7-Variante

[growth]
growth_factor_per_iteration   = 0.30          # g
max_new_wallets_per_iter      = 1000
charge_eur_to_agc_ratio       = 1.0

[consensus]
n_validators                  = 1
poh_tick_ms                   = 400
ticks_per_slot                = 64
max_txs_per_tick              = 256
leader_rotation               = "round_robin"

[iteration]
iterations_per_year           = 12
ticks_per_iteration           = 6566400       # 30.4 Tage @ 400ms
# Im Sim-Modus override

[storage]
db_path                       = "./data/agora.sled"
snapshot_path                 = "./snapshots/"
snapshot_per_iteration        = true
seed_path                     = "./seeds/"

[security]
operator_pubkey_env           = "AGORA_OPERATOR_PUBKEY"
operator_secret_env           = "AGORA_OPERATOR_SECRET"
require_captcha_proof         = true
rate_limit_wallet_per_ip_per_day = 5
rate_limit_tx_per_wallet_per_minute = 60

[simulation]
enabled                       = false
speed_factor                  = 1000          # 1000x Echtzeit
seed_file                     = "seeds/default.json"
auto_generate_users           = true
users_per_iteration_mean      = 80            # Erwartungswert
users_per_iteration_stddev    = 15
tx_per_active_wallet_mean     = 5
```

---

## Teil G — REST-API-Spezifikation

### G.1 Authentifizierungs-Modi

1. **Operator-signiert**: HTTP-Header `X-Operator-Id`, `X-Operator-Signature`. Signatur über kanonischen Request-Body + Timestamp.
2. **User-signiert**: Signatur als Feld in Request-Body.
3. **Offen**: keine Authentifizierung (Read-Endpoints).
4. **Admin-Master-signiert**: wie Operator, aber nur Genesis-Operator akzeptiert.

### G.2 Endpunkte

| Methode | Pfad | Auth | Zweck |
|---|---|---|---|
| POST | `/api/v1/wallet` | Operator | Wallet erstellen |
| POST | `/api/v1/charge` | Operator | EUR → AGC mint |
| POST | `/api/v1/transaction` | User | Transfer |
| POST | `/api/v1/burn` | User | Burn |
| GET | `/api/v1/wallet/{id}` | Offen | Wallet-Info |
| GET | `/api/v1/wallet/{id}/transactions` | Offen | TX-Liste |
| GET | `/api/v1/transaction/{id}` | Offen | TX-Detail |
| GET | `/api/v1/iteration/current` | Offen | Aktuelle Iteration |
| GET | `/api/v1/iteration/{id}` | Offen | Iteration-Detail |
| GET | `/api/v1/stats` | Offen | Globale Metriken |
| GET | `/api/v1/stats/gini` | Offen | Gini-Historie |
| GET | `/api/v1/stats/supply` | Offen | Supply-Historie |
| WS | `/api/v1/events` | Offen | Live-Event-Stream |
| GET | `/admin/parameters` | Admin | Aktuelle Parameter |
| PUT | `/admin/parameters` | Admin | Parameter ändern |
| POST | `/admin/simulation/start` | Admin | Simulation starten |
| POST | `/admin/simulation/stop` | Admin | Simulation stoppen |
| GET | `/admin/simulation/status` | Admin | Sim-Status |
| POST | `/admin/snapshot` | Admin | Manueller Snapshot |
| POST | `/admin/operator` | Admin | Operator hinzufügen (v2.0) |

### G.3 Response-Format

Alle Antworten JSON, Standardstruktur:

```
{
  "success": bool,
  "data": { ... } | null,
  "error": { "code": "...", "message": "..." } | null,
  "tick": 12345,
  "iteration": 7
}
```

### G.4 Fehler-Codes

| Code | HTTP | Bedeutung |
|---|---|---|
| `INVALID_SIGNATURE` | 401 | Signatur-Verifikation fehlgeschlagen |
| `INVALID_NONCE` | 400 | Nonce stimmt nicht überein |
| `INSUFFICIENT_BALANCE` | 400 | Saldo zu niedrig |
| `WALLET_NOT_FOUND` | 404 | Unbekannte Wallet-ID |
| `OPERATOR_UNAUTHORIZED` | 403 | Operator nicht aktiv |
| `RATE_LIMIT_EXCEEDED` | 429 | Limit überschritten |
| `ITERATION_IN_PROGRESS` | 409 | Iteration läuft, TX blockiert |
| `INTERNAL_ERROR` | 500 | Server-Fehler |

### G.5 Beispiel: Wallet-Erstellung

**Request:**
```
POST /api/v1/wallet
X-Operator-Id: 7a3f...
X-Operator-Signature: 9c1b...

{
  "public_key": "base64(...)",
  "captcha_token": "...",
  "timestamp": 1730000000000
}
```

**Backend-Verarbeitung:**
1. Operator-Signatur über `(public_key, captcha_token, timestamp)` verifizieren.
2. Operator-Status prüfen (`active = true`).
3. Captcha-Token gegen Captcha-Service prüfen (oder Mock im Prototyp).
4. Timestamp-Drift prüfen (max. 60 s).
5. `wallet_id = BLAKE3(public_key)` berechnen.
6. Kollision prüfen.
7. Wallet erstellen, in nächste TX-Liste aufnehmen, an Leader weiterleiten.
8. Auf Inclusion in PoH-Entry warten (oder asynchron 202 zurückgeben mit `tx_id`).

**Response 201:**
```
{
  "success": true,
  "data": {
    "wallet_id": "base64(...)",
    "balance": 10000000,
    "created_at_tick": 12345
  },
  "tick": 12345,
  "iteration": 7
}
```

---

## Teil H — Persistenz-Schema

### H.1 sled Tree-Struktur

| Tree-Name | Key | Value | Zweck |
|---|---|---|---|
| `wallets` | `wallet_id` | `Wallet` (bincode) | Live-Wallet-State |
| `operators` | `operator_id` | `Operator` | Operator-Registry |
| `validators` | `validator_id` | `Validator` | Validator-Registry |
| `transactions` | `tx_id` | `Transaction` | TX-Archiv |
| `poh_log` | `tick` (u64 BE) | `PohEntry` | PoH-Stream |
| `tx_by_wallet` | `wallet_id || tick` | `tx_id` | Sekundärindex |
| `iterations` | `iteration_id` | `IterationCommit` | Iteration-Archiv |
| `system` | `"state"` | `SystemState` | Global Singleton |
| `system` | `"parameters"` | `SystemParameters` | Config |
| `nonce_index` | `wallet_id` | `u64` | Aktueller erwarteter Nonce |

### H.2 Snapshots

Pro Iteration nach `IterationCommit`:

```
snapshots/iter_{N}.json
{
  "iteration_id": N,
  "tick": ...,
  "timestamp": ...,
  "parameters": { ... },
  "system_state": { ... },
  "wallets": [ ... ],
  "recent_transactions": [ ... ],   // optional, letzte K
  "poh_anchor": "hash..."
}
```

### H.3 Seed-Format

```
seeds/{name}.json
{
  "metadata": {
    "name": "scenario_100_nodes",
    "description": "...",
    "created_at": "..."
  },
  "parameters": { ... gesamte SystemParameters ... },
  "genesis_operator": {
    "pubkey": "base64",
    "secret_key_encrypted": "..."   // optional, sonst aus ENV
  },
  "initial_validators": [ ... ],
  "initial_wallets": [
    { "pubkey": "...", "initial_balance": 10000000, "created_at_iteration": 0 }
  ],
  "scripted_events": [              // optional
    { "at_iteration": 3, "action": "spawn_wallets", "count": 50 },
    { "at_iteration": 5, "action": "transfer_burst", "n_txs": 1000 }
  ]
}
```

---

## Teil I — Iteration-Engine (Pseudocode-Spezifikation)

```
function execute_iteration(iteration_id):
    # 1. Iteration-Lock setzen (blockiert neue User-TXs)
    set_iteration_lock(true)
    
    # 2. Pending TXs leer laufen lassen (Mempool flushen)
    wait_until_mempool_empty(timeout = 60s)
    
    # 3. Snapshot
    snapshot_supply = sum(w.balance for w in all_wallets)
    snapshot_n     = count(all_wallets)
    snapshot_gini  = compute_gini(all_wallets)
    
    # 4. Inflation seit letzter Iteration berechnen
    if iteration_id > 0:
        I = (snapshot_supply - prev_supply) / prev_supply
    else:
        I = 0
    
    # 5. Burn-Rate anpassen
    β = clip(β₀ + k_β · (I - I*), β_min, β_max)
    
    # 6. Penalty pro Wallet
    b_max = max(w.balance for w in all_wallets)
    penalties = {}
    for w in all_wallets:
        penalties[w.id] = floor(r · w.balance · ln(1 + w.balance/s) / ln(1 + b_max/s))
    
    P = sum(penalties.values())
    
    # 7. Auto-Kalibrierung (optional)
    target_P = τ · snapshot_supply
    if abs(P - target_P) / target_P > 0.2:
        log_warning("Penalty deviation > 20% from target")
    
    # 8. Burn und Pools
    B = floor(β · P)
    F_from_penalty = min(N_new · s, φ · P)
    R = P - B - F_from_penalty
    
    # 9. Aktivitäts-Scores aktualisieren
    for w in all_wallets:
        raw = 1 if (w.iteration_tx_count ≥ t_min 
                    and len(w.iteration_counterparties) ≥ c_min) else 0
        w.activity_score = λ · raw + (1-λ) · w.activity_score
        α_i = α_min + (α_max - α_min) · w.activity_score
    
    # 10. Redistribution
    total_α = sum(α_i for all wallets)
    rewards = {}
    for w in all_wallets:
        rewards[w.id] = floor(R · α_i / total_α)
    
    # 11. Rundungs-Residual
    residual = R - sum(rewards.values())
    # → an Genesis-Wallet oder zusätzlich burnen
    
    # 12. Balances aktualisieren
    for w in all_wallets:
        w.balance = w.balance - penalties[w.id] + rewards[w.id]
        w.iteration_tx_count = 0
        w.iteration_counterparties.clear()
    
    # 13. Neue Wallets erzeugen (aus Faucet-Anträgen oder geplanten Events)
    N_new = min(floor(g · snapshot_n), N_max_cap)
    create_n_new_wallets(N_new)  # erhält jeweils s aus Faucet-Pool
    
    # 14. Faucet-Mint falls Unterdeckung
    F_needed = N_new · s
    F_from_mint = max(0, F_needed - F_from_penalty)
    if F_from_mint > 0:
        emit_faucet_mint(F_from_mint)
    
    # 15. IterationCommit-TX erzeugen, signieren, in PoH einfügen
    commit = IterationCommit { ... }
    sign_and_broadcast(commit)
    
    # 16. Snapshot schreiben
    write_snapshot(iteration_id + 1)
    
    # 17. State aktualisieren
    current_iteration += 1
    iteration_started_at = current_tick
    
    # 18. Iteration-Lock lösen
    set_iteration_lock(false)
```

### I.1 Atomarität

Schritte 6–14 müssen atomar sein. Implementierung: `sled::Batch` oder Transaktion. Bei Fehler: vollständiger Rollback, Iteration nicht inkrementieren.

### I.2 Trigger

Iteration startet automatisch bei:

$$\text{current\_tick} \geq \text{iteration\_started\_at} + \text{ticks\_per\_iteration}$$

Im Simulationsmodus: manuell oder beschleunigt.

---

## Teil J — Simulations-Modul

### J.1 Modi

1. **Replay-Modus**: Lädt Seed-JSON, spielt deterministisch ab, generiert Snapshots.
2. **Synthetic-Modus**: Generiert zufällige Nutzer/TXs nach statistischen Parametern.
3. **Hybrid-Modus**: Seed-Startzustand + synthetische Folge-Events.

### J.2 Statistische TX-Generierung

Pro Iteration:
- Anzahl neuer Wallets: $N_{\text{new}} \sim \mathcal{N}(\mu_{\text{new}}, \sigma_{\text{new}})$, abgeschnitten ≥ 0.
- TXs pro aktivem Wallet: $k_i \sim \text{Poisson}(\mu_{\text{tx}})$.
- TX-Betrag: $a \sim \text{Lognormal}(\mu_a, \sigma_a)$, abgeschnitten an Saldo.
- Empfänger: gewichtet zufällig (Preferential Attachment optional).

Deterministische Seeds: Alle RNGs aus `ChaCha20Rng` mit konfigurierbarem Seed.

### J.3 Zeitbeschleunigung

`speed_factor = K` → Tick-Dauer wird auf `poh_tick_ms / K` reduziert, Mindestwert 1 ms.
1 Jahr Realzeit = 12 Iterationen × ~30 Tage:
- bei `K = 1000`: ~8.8 Stunden
- bei `K = 100000`: ~5.3 Minuten

### J.4 Use-Case-Szenarien (auszuliefern als Seeds)

| Szenario | Beschreibung |
|---|---|
| `default_100_nodes` | 100 Wallets (10 Validator), Standard-Parameter, 24 Iterationen |
| `high_inflation` | Charge-Burst, prüft Burn-Anpassung |
| `wealth_concentration` | Initial 80/20-Verteilung, prüft Gini-Reduktion |
| `low_activity` | Wenige TXs, prüft Aktivitäts-Penalty |
| `growth_explosion` | g = 1.0, prüft Faucet-Skalierung |
| `attack_sybil` | Operator-Key kompromittiert (in Simulation), Resilienz prüfen |

### J.5 Metrik-Output

Pro Iteration in CSV/JSON:
- Tick, IterationId, Timestamp
- Supply, n_wallets, n_active
- Gini, Top10%-Anteil, Median-Saldo
- Penalty-Total, Burn-Total, Reward-Total
- Anzahl neuer Wallets, Anzahl TXs in Iteration
- Aktuelle Burn-Rate β, Inflation I

---

## Teil K — Sicherheits-Modell

### K.1 Bedrohungen und Gegenmaßnahmen

| Bedrohung | Gegenmaßnahme |
|---|---|
| Sybil-Attacke (Wallet-Spam) | Operator-Signatur erforderlich; Captcha; IP-Rate-Limit |
| Operator-Key-Leck | Audit-Log; Rate-Limit auch auf Operator-Ebene; v2.0: Threshold-Sig |
| Replay-Angriff | Per-Wallet-Nonce; Timestamp-Window |
| Double-Spend | PoH stellt totale Ordnung her; Validator prüft Nonce vor Inclusion |
| TX-Manipulation | Ed25519-Signatur, BLAKE3-Integritäts-Hash |
| Validator-Fehlverhalten | v1.0: Trust-Annahme; v2.0: BFT + Slashing |
| DoS auf REST | Rate-Limit pro IP, Connection-Limit, Body-Size-Limit |
| Activity-Farming | Distinct-Counterparty-Forderung; Mindest-TX-Betrag; Graph-Analyse (v2.0) |
| Penalty-Umgehung durch Splitting | Logarithmische Kurve ohne Schwelle → Splitting bringt geringen Vorteil, Operator-Permission begrenzt Wallet-Anzahl |

### K.2 Master-Key-Schutz

- Im Prototyp: aus ENV (`AGORA_OPERATOR_SECRET`), nicht im Code.
- Empfehlung Produktion: HashiCorp Vault, AWS KMS, oder Hardware-HSM.
- Niemals in Logs, niemals in Fehlermeldungen.
- Audit-Log für jede Operator-Signatur (Hash der signierten Payload, Timestamp).

### K.3 Captcha-Integration

Externe Validierung. Interface:

```
fn verify_captcha(token: String) -> Result<bool>
```

Mock-Implementierung im Prototyp: akzeptiert beliebigen nicht-leeren Token im Dev-Modus, aktiv im Production-Modus (Cloudflare Turnstile oder hCaptcha empfohlen).

---

## Teil L — Tech-Stack Festlegung

| Schicht | Crate / Tool | Begründung |
|---|---|---|
| Async Runtime | `tokio` | Standard |
| Web-Framework | `axum` | Modern, type-safe, tower-kompatibel |
| Serialisierung (Wire) | `serde_json` (REST), `bincode` (intern), `borsh` (kanonisch für Signaturen) | bincode + borsh deterministisch |
| Hashing | `blake3` | Schnell, modern |
| Signaturen | `ed25519-dalek` v2 | Standard, audited |
| DB | `sled` | Embedded, einfache API |
| P2P | `libp2p` (Gossipsub + Kademlia) | Industriestandard |
| RPC zwischen Validators | `tonic` (gRPC) | Performant, strukturiert |
| Logging | `tracing` + `tracing-subscriber` | Strukturiert |
| Metriken | `prometheus` | Standard für Monitoring |
| Konfiguration | `config` + TOML | Flexibel |
| CLI | `clap` v4 | Standard |
| Tests | `proptest` (Property), `criterion` (Bench), `tokio-test` | Vollabdeckung |
| Frontend | Leptos (Rust) ODER React + Recharts | Wahl pro Team-Skill |
| WebSocket | `axum` integriert | Native Support |
| Statistik | `statrs` | Gini, Verteilungen |
| RNG (deterministisch) | `rand_chacha` | Reproduzierbare Sims |
| Captcha | `hcaptcha` Client oder eigener HTTP-Call | — |

---

## Teil M — Workspace-Struktur

```
agora-coin/
├── Cargo.toml                    [workspace]
├── README.md
├── docs/
│   ├── spec.md                   diese Datei
│   ├── architecture.md
│   └── runbook.md
├── crates/
│   ├── agora-types/              Primitive, Datenmodell, Serialisierung
│   ├── agora-crypto/             Ed25519, BLAKE3, Merkle
│   ├── agora-storage/            sled-Wrapper, Schema, Snapshots
│   ├── agora-poh/                PoH-Generator, Verifier
│   ├── agora-state/              State-Machine, TX-Validierung
│   ├── agora-economy/            Penalty/Burn/Redistribution/Iteration
│   ├── agora-consensus/          Leader-Rotation, Block-Production
│   ├── agora-network/            libp2p, gRPC zwischen Validators
│   ├── agora-rest/               axum REST + WebSocket
│   ├── agora-simulator/          Synthetic-TX-Generator, Replay
│   ├── agora-config/             TOML-Parsing, Validierung
│   └── agora-cli/                Binaries: agora-node, agora-sim, agora-admin
├── seeds/
│   ├── default_100_nodes.json
│   ├── high_inflation.json
│   └── ...
├── snapshots/                    Runtime-Output
├── data/                         sled-DBs
├── web-gui/
│   ├── src/                      Leptos oder React
│   └── package.json (falls React)
├── scripts/
│   ├── gen-keys.sh
│   ├── run-sim.sh
│   └── plot-metrics.py
└── tests/
    ├── integration/
    └── e2e/
```

### M.1 Inter-Crate-Abhängigkeiten

```
agora-types     ← (kein Dep)
agora-crypto    ← agora-types
agora-storage   ← agora-types, agora-crypto
agora-poh       ← agora-types, agora-crypto
agora-state     ← agora-types, agora-crypto, agora-storage
agora-economy   ← agora-types, agora-state
agora-consensus ← agora-types, agora-poh, agora-state, agora-economy
agora-network   ← agora-types, agora-consensus
agora-rest      ← agora-types, agora-state, agora-consensus, agora-network
agora-simulator ← alle obigen
agora-config    ← agora-types
agora-cli       ← alle obigen
```

---

## Teil N — Test-Strategie

### N.1 Unit-Tests
Pro Crate, Coverage-Ziel ≥ 80 %.

### N.2 Property-Tests (proptest)

Invarianten zu prüfen:
- Supply-Konservierung (außer bei explizitem Mint/Burn).
- Nonce-Monotonie.
- Signatur-Roundtrip.
- Penalty-Funktion: monoton, Penalty(0) = 0.
- Gini ∈ [0, 1].
- Iteration: Pre-Supply - Burn + Mint = Post-Supply.

### N.3 Integration-Tests

- Wallet → TX → Inclusion → Balance-Update.
- Iteration mit synthetischer Population.
- Operator-Signatur-Verifikation.
- Replay-Determinismus aus Seed.

### N.4 E2E-Tests

- Vollständiger REST-Lifecycle.
- Multi-Validator-Setup (mit mehreren Prozessen / Docker).
- 1-Jahr-Simulation in 10 Minuten Wallclock.

### N.5 Benchmarks (criterion)

- TX-Validation-Durchsatz (Ziel ≥ 10.000 TX/s pro Validator).
- PoH-Tick-Latenz (Ziel < 100 ms).
- Iteration-Ausführung (≤ 5 s für 10.000 Wallets).

---

## Teil O — Roadmap & Phasenplan

### Phase 1 — Core-Foundation
`agora-types`, `agora-crypto`, `agora-storage` mit vollständigen Tests.

### Phase 2 — State-Machine Single-Node
`agora-state` + `agora-economy`. CLI-Tool zum manuellen TX-Apply. Iteration funktional.

### Phase 3 — PoH + REST
`agora-poh` + `agora-rest`. Single-Validator-Node lauffähig, REST API komplett.

### Phase 4 — Simulator
`agora-simulator` mit Seed-Replay und Synthetic-Mode. Erste Wirtschaftsdaten generieren.

### Phase 5 — Web-GUI
Parameter-Panel, Live-Charts (Gini, Supply, Inflation), TX-Explorer, Iteration-Timeline.

### Phase 6 — Multi-Validator
`agora-consensus` + `agora-network`. 4-Node-Setup, Round-Robin Leader.

### Phase 7 — Härtung
Rate-Limits, Captcha-Integration, Operator-Signatur-Audit, Monitoring.

### Phase 8 (v2.0) — BFT-Konsens, Threshold-Signatur, mehrere Operatoren
Außerhalb Prototyp-Scope.

---

## Teil P — Offene Punkte & noch zu treffende Entscheidungen

1. **EUR-Custody bei Charge**: Wer hält die Fiat-Mittel? Treuhandkonto bei welcher Bank? Regulatorischer Status (E-Geld vs. Utility-Token vs. Wertpapier nach MiCAR)? — *Außerhalb Prototyp, aber rechtzeitig zu klären.*

2. **Wallet-Recovery**: Privatschlüssel-Verlust = AGC-Verlust. Soll ein optionaler Recovery-Mechanismus über Operator existieren (mit klarem Trust-Trade-off)?

3. **Inaktive Wallet-Garbage-Collection**: Nach wie vielen Iterationen ohne Aktivität wird ein Wallet archiviert (nicht gelöscht)?

4. **Maximale TX-Größe / Memo-Feld**: Soll ein optionales `memo`-Feld unterstützt werden? Größenlimit?

5. **GUI-Stack-Entscheidung**: Leptos (Rust-nativ) oder React?

6. **Captcha-Provider**: Cloudflare Turnstile, hCaptcha, oder eigener Service?

7. **Charge-Mindest-/Höchstbeträge**: AML-Schwellen?

8. **Snapshot-Retention**: Wie viele Snapshots aufbewahren?

9. **Validator-Onboarding-Prozedur**: Im v1.0 nur Genesis. Aber wie genau in der Config-Datei eintragbar für Simulation mit mehreren Validators?

10. **Iteration-Lock-Verhalten**: Sollen User-TXs während der Iteration in einem Queue gepuffert oder mit `409 Conflict` abgelehnt werden?

---

**Ende der Spezifikation v1.0**

Dieses Dokument ist vollständig genug, um an ein Code-Generierungssystem oder Entwicklungsteam zur Implementierung übergeben zu werden. Alle mathematischen Formeln sind explizit, alle Datentypen sind definiert, alle Endpunkte sind spezifiziert. Klärung der Punkte in Teil P ist vor finalem Produktionsstart erforderlich, blockiert aber den Prototyp-Build nicht.