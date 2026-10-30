# GTA Online and friends

GTA Online's freemode runs between GLUE 5 players, with no Rockstar
services and no server of ours: the players' own devices find each other
and one of them hosts the session, as on a console.

## Playing online

1. On the start page, open **GTA ONLINE** and set the name other players
   see.
2. Press **Play**. You join the lobby of everyone playing; the game's own
   matchmaking finds a public session or hosts one.

The pause menu's other sessions (**invite only**, **crew**, **closed
friends**, **solo**) each get a lobby of their own, reached only through
an invite or, for closed friends, by your friends.

From **Story Mode** and the sandbox GTA Online is a choice away, as on
PC: the pause menu's **ONLINE** tab (**Play GTA Online** and its
sessions, **Join Friends**, the creators); its **FRIENDS** tab shows your
friends and who is playing. A friend's invite, or **Join** on the Friends
page, takes you to your friend's session too: the game asks whether to
leave, loads your GTA Online character and goes there. With no character
yet you make one first: the invite waits through the creator (and the
tutorial, if you play it) and then takes you to your friend. Touch play
aims with assistance, a friend with a mouse aims freely: you play together
all the same, with no warning about the targeting mode. The story itself
stays offline: only those menus and the way into GTA Online see the
network.

Your characters are kept on your device (the [user folder](getting-started.md#where-your-data-is)'s
`cloud`), not shared with Rockstar.

## Friends

The **Friends** page (♡ on the start page, Home, or Back + RB on a
controller) works like Social Club's overlay: your friends and who is
playing, friend requests, game invites, and the players you met. Friends
see when you play, join you, and invite you, from the game's own menus
too. Notifications come up at the top right: a friend online, a request,
an invite (Settings › Game turns them off).

To add a friend:

- **With a friend code.** The Find tab shows yours: 16 characters to give
  to a friend, who asks you with it. You say yes or no. A new code retires
  the old one.
- **With someone you met online.** The players you met, on the Find tab,
  or **Send Friend Request** on a player in the pause menu, while you are
  both in the same lobby.

## Privacy and security

- Friends talk to each other over an **end-to-end encrypted** channel (the
  Noise protocol: X25519, ChaCha20-Poly1305, BLAKE2s). Nobody else can read
  or forge what they say, and a device answers no one but its friends.
- A friend code proves who is behind it: knowing someone's code is not
  enough to answer for them.
- Like any player in the same session, a friend sees your Internet
  address.
- A device keeps and answers in full only someone who shows it receives
  at its address: nobody can use the players to flood someone else's.
- Your key, code and friends are in `friends.json` in the user folder,
  readable by you only.

## How it works

- **A virtual LAN.** The game plays online as it would on a local network:
  its sockets carry their datagrams over one UDP socket per player (port
  47000 to 47015, or another if all are taken).
- **Finding players.** On the same network, by broadcast. Over the
  Internet, through the BitTorrent DHT, where each player announces itself
  under the lobby's key. Behind NATs, both sides send until the NATs let
  them through; players who cannot reach each other go through one who
  can.
- **Finding friends.** Two friends meet on the DHT and the local network
  under a key only the two of them can compute (from their keys'
  Diffie-Hellman secret, changing every 6 hours).
- **In the game.** The pause menu's friends are yours; invites to a session
  or a job, and their answers, travel over the encrypted channel and are
  accepted only from that friend. Join on a friend's card, an accepted
  invite or the pause menu's **Join Friends** take you to the friend's
  lobby and session.

The code is in `crates/glue5-runtime/src`: `vnet.rs`, `p2p.rs`, `dht.rs`,
`online.rs` and `friends/`.
