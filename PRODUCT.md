# Product

PrivCord is a private messenger with no company in the middle: messages are sealed on the sender's phone, carried by relays that cannot read them, and the apps work without accounts or phone numbers. Anyone can run their own relay (and TURN and Nostr) with `privcord-server`.

## Users of the server's admin panel

- **A person running a server for themselves, family or friends**, often a beginner on a home machine or a small VPS. They open the panel rarely: to get the addresses for the apps, make a token for a friend, or check it is healthy. They should never need to know what a mailbox quota is.
- **An experienced operator** who wants limits, rules, moderation and several admins with roles.

## What the panel is for

Connect the apps to this server, control who may use it, keep it healthy, and clear data on purpose. The important and irreversible things (clear stored messages, clear events) are at the top of Settings, not buried.

## Tone

Plain words. Say what a control does, not what it is called in the code. Honest about risk (an open server, a self-signed certificate). No marketing voice inside the tool.

## Constraints

- Served by the server program itself: plain HTML, CSS and JavaScript, no framework, no build step, nothing loaded from anywhere else (strict content-security-policy), fonts and icons embedded.
- Works on a phone and on a desk, in light and dark, by keyboard and with a screen reader.
- Beginners see little; an Advanced switch shows the rest.

## Anti-references

Generic SaaS dashboards: grids of identical stat cards, neon-on-black "terminal" styling, glowing edges, emoji icons, modals for everything.
