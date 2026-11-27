# Trying the PrivCord server on your own machine or a VPS

A short checklist. Real networks decide the TURN questions, and nothing here has run on one yet (the repository's NAT lab
emulates NATs on one machine; it is not the internet). Tell the project what you find.

## 1. Get it

Run the **Server binaries** workflow from the repository's Actions tab (tick *publish* to put the files on the **Server**
release), download the file for your system, and make it executable (`chmod +x privcord-server-linux-x86_64`).

## 2. Set it up (about five minutes)

```sh
./privcord-server init --turn          # writes privcord-server.toml
```

Edit the file: in `[admin]` set `enabled = true`; in `[turn]` set `tcp_listen = "0.0.0.0:3478"`. Then:

```sh
./privcord-server doctor               # it tells you what to open and what is wrong
./privcord-server serve                # prints the admin address and your admin token ONCE: save it
```

On a **VPS**: allow in its firewall TCP 8787, UDP 3478, TCP 3478 and UDP 49152-49400.
At **home**: forward those same ports on your router to the machine. If `doctor` says your address is shared with other
customers (carrier-grade NAT), a home server cannot work; use a VPS.

## 3. What to check, in order

1. Open the admin panel (from the server itself, or with an SSH tunnel) and sign in. **Overview** lists the addresses for the apps.
2. In the Access tab, make a token. Put `ws://YOUR-IP:8787/?token=THE-TOKEN` as the relay in the apps on two phones.
3. Send a message between the two phones **on different networks** (one on Wi-Fi, one on mobile data). It should arrive.
4. Close the app on one phone for ten minutes, send messages to it, open it: they should be waiting (needs `store_and_forward`, on by default).
5. Voice or a file between the two phones on different networks (this is TURN). In the apps' TURN settings give
   `turn:YOUR-IP:3478` and a credential from `http://YOUR-IP:8787/turn`.
6. If one phone is on a network that blocks UDP (an office, a school), try `turn:YOUR-IP:3478?transport=tcp`.
7. Restart the server (`Ctrl-C`, run again): the token, the access tokens and the stored messages should still be there.

## 4. If something fails

Run the server with `PRIVCORD_LOG=debug ./privcord-server serve` and send the output (it contains addresses, so read it first),
and the output of `./privcord-server doctor`. In the apps, use Settings, the diagnostic log.
