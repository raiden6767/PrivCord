import Gallery, { SHOTS } from './Gallery.jsx';
import DotGrid from './rb/DotGrid.jsx';
import Magnet from './rb/Magnet.jsx';
import StarBorder from './rb/StarBorder.jsx';
import { Icon, P, FAQ, COMPARE } from './data.jsx';

const INK = '#0a0a0a';
const YELLOW = '#ffe500';

/* The menu pages: each one goes deeper than its short section on the home page. */

export function Head({ eyebrow, title, lead }) {
  return (
    <section className="page-head">
      <div className="wrap">
        <p className="eyebrow">{eyebrow}</p>
        <h1>{title}</h1>
        <p className="lead">{lead}</p>
      </div>
    </section>
  );
}

export function Next({ Go, items }) {
  return (
    <section className="page-next">
      <div className="wrap">
        <p className="eyebrow">Keep going</p>
        <ul>{items.map(([to, t]) => <li key={to}><Go className="more" to={to}>{t} →</Go></li>)}</ul>
      </div>
    </section>
  );
}

const FEATURE_GROUPS = [
  { t: 'Private by design', i: 'lock', items: [
    ['Direct, end-to-end encrypted', "Messages are encrypted on your device and travel straight to your friend's over a WebRTC link. The handshake mixes ML-KEM-768 with P-256 so it is built to hold up against future quantum computers, every message uses its own key, and messages are padded so their length tells little."],
    ['No account, no phone number', 'Your identity is a 12-word recovery phrase made on your device. There is nothing to sign up for and nobody holds your account.'],
    ['Keys you pinned', 'When you add a friend, both of you pin each other\'s keys. Every later link is checked against them, and each chat shows whether it is sealed that way.'],
    ['Hide my IP and Tor mode', 'Friends normally see the address your link comes from. Turn on "Hide my IP from friends" to go through a relay only, or use Tor mode, at the cost of speed.'],
  ] },
  { t: 'You stay in control', i: 'key', items: [
    ['Lock and wipe', 'Lock the app with Windows Hello or your fingerprint, block screenshots on a device, and wipe everything in one tap.'],
    ['Passphrase-locked backups', 'Backups are encrypted with a key derived from your passphrase (Argon2id, 12 characters minimum), including files and old groups.'],
    ['Privacy checkup', 'One screen lists what is weak (no lock, previews showing, anyone can message you) and fixes each in a tap.'],
    ['Network transparency', 'A plain list of every host the app is allowed to talk to, so you can check it yourself.'],
    ['Diagnostics that never carry content', 'The export holds counts and yes/no answers only: no text, names, keys or ids.'],
  ] },
  { t: 'Everyday chat', i: 'users', items: [
    ['Groups of up to 10', 'Add Ben, Ben adds Cat, and you and Cat connect directly without sharing any code. Only an admin changes the group.'],
    ['Replies, reactions, search, delete for everyone', 'The things you expect, kept on your device.'],
    ['Files, photos and voice notes', 'Up to 50 MB, sent straight to the other device. Photos and videos preview in the chat, voice notes show a waveform and play at your speed.'],
    ['Undo send and send later', 'A few seconds to take a message back. Schedule one with quick picks (in an hour, this evening, tomorrow morning) in your own time zone.'],
    ['Pin, archive, mute, quiet hours', 'Pin chats, archive the noisy ones, mute for an hour, 8 hours, until tomorrow or for good, mark a chat silent, and set quiet hours. They follow you to every device.'],
    ['Delivery details', 'See, per device, whether a message was delivered or read, and which route it took.'],
  ] },
  { t: 'Many devices, any network', i: 'laptop', items: [
    ['One account on your phone and PC', 'Link a new device: it shows a QR code, your phone scans it, and you type six digits on the new device. A photo of the code alone is useless. The account key never leaves your first device.'],
    ['Lose a device safely', 'Remove it from another device (takes effect in 24 hours, any device can cancel) or instantly with your recovery phrase. Keys rotate afterwards.'],
    ['Works on bad networks', 'Optional signaling relays, STUN and TURN find a path for you, and devices on the same Wi-Fi link directly.'],
    ['Bluetooth with no internet (Android, experimental)', 'An optional mesh passes sealed messages through nearby phones, up to 7 hops. A banner tells you when a chat is going that way. Tested in simulation so far, not on real radios.'],
  ] },
  { t: 'On Windows and Android', i: 'mon', items: [
    ['Windows', 'A quick-send window on Ctrl+Alt+D (it stays locked while the app lock is on), an unread badge on the tray icon and taskbar, and drop files anywhere to choose who gets them.'],
    ['Android', 'Biometric lock, reply from the notification, and background delivery tuned for phones that stop apps aggressively.'],
  ] },
];

const NUMBERS = [
  ['Group size', '10 people (a full mesh, so 9 links per device)'],
  ['File size', '50 MB, device to device'],
  ['Bluetooth mesh', 'Up to 7 hops, Android only, experimental'],
  ['Six-word invite', 'Valid for 15 minutes while the app stays open'],
  ['Device removal', 'Takes effect after 24 hours (cancel any time), or at once with the recovery phrase'],
  ['Linking a device', 'Six digits, three tries'],
  ['Backup passphrase', '12 characters minimum (Argon2id)'],
  ['Quick-send window (Windows)', 'Ctrl+Alt+D, locked while the app lock is on'],
  ['Platforms', 'Windows 10/11 and Android'],
];

export function FeaturesPage({ Go }) {
  return (
    <>
      <Head eyebrow="Features" title="Everything PrivCord does, in detail" lead="The short list is on the home page. This is the longer one: what each feature does, and where it stops." />
      {FEATURE_GROUPS.map((g) => (
        <section className="page-sec" key={g.t}>
          <div className="wrap">
            <h2 className="group-h"><Icon d={P[g.i]} />{g.t}</h2>
            <div className="fgrid">
              {g.items.map(([t, p]) => (
                <article className="fcard plain" key={t}><h3>{t}</h3><p>{p}</p></article>
              ))}
            </div>
          </div>
        </section>
      ))}
      <section className="page-sec">
        <div className="wrap">
          <h2 className="group-h">The numbers</h2>
          <div className="tablewrap"><table>
            <thead><tr><th>Thing</th><th>Limit</th></tr></thead>
            <tbody>{NUMBERS.map(([a, b]) => <tr key={a}><td>{a}</td><td>{b}</td></tr>)}</tbody>
          </table></div>
        </div>
      </section>
      <section className="page-sec">
        <div className="wrap">
          <h2 className="group-h">Not there yet</h2>
          <p className="sub">So you know before you install: no iPhone, macOS or Linux app; no independent audit; no resumable transfers after a restart; silent chats still play the default sound in a Windows toast; and the Bluetooth mesh is experimental.</p>
        </div>
      </section>
      <Next Go={Go} items={[['how', 'How it works'], ['screens', 'Screens'], ['download', 'Download']]} />
    </>
  );
}

const TRAVEL = [
  ['You add a friend', 'A QR code or six words carries each side\'s public key. Both of you pin the other\'s key; from now on only that key counts.'],
  ['Your devices find each other', 'They post small sealed notes through optional relays (public Nostr relays or your own) saying how to connect. The relay cannot read them.'],
  ['A direct link opens', 'The two devices connect over WebRTC. STUN helps each learn its public address; on strict networks TURN relays the already-encrypted traffic. Then messages flow device to device.'],
  ['If there is no route', 'Messages wait on your device and send when you reconnect. On Android, nearby phones can carry sealed messages over Bluetooth with no internet.'],
];
const SEES = [
  ['Your friend', 'Your messages, and (unless you hide it) the address your link comes from'],
  ['A signaling relay', 'Sealed blobs, and the addresses of devices that talk to it'],
  ['A STUN server', 'The public address you ask it about'],
  ['A TURN server', 'Encrypted traffic and who is talking to whom, never the content'],
  ['Anyone else', 'Nothing readable'],
];

export function HowPage({ Go }) {
  return (
    <>
      <Head eyebrow="How it works" title="From install to your first message" lead="Three steps to get going, and then what actually happens when you press send." />
      <section className="page-sec">
        <div className="wrap">
          <h2 className="group-h">Three steps</h2>
          <ol className="longsteps">
            <li><b>1</b><div><h3>Install and write down 12 words</h3><p>The recovery phrase is your account. It is never stored anywhere else, so keep it safe: the same words restore you on a new device. The app asks you to confirm you wrote it down.</p></div></li>
            <li><b>2</b><div><h3>Add a friend</h3><p>Show your QR code or read out six words. They scan or type them, you tap Add. Both of you now have each other's keys pinned. Groups work the same way, and members connect to each other automatically.</p></div></li>
            <li><b>3</b><div><h3>Chat</h3><p>Messages go device to device. If your friend is offline they wait, and send when you reconnect. Files, photos and voice notes go the same way.</p></div></li>
          </ol>
        </div>
      </section>
      <section className="page-sec">
        <div className="wrap">
          <h2 className="group-h">How a message travels</h2>
          <ol className="longsteps">
            {TRAVEL.map(([t, p], i) => (<li key={t}><b>{i + 1}</b><div><h3>{t}</h3><p>{p}</p></div></li>))}
          </ol>
        </div>
      </section>
      <section className="page-sec">
        <div className="wrap">
          <h2 className="group-h">What each helper can see</h2>
          <div className="tablewrap"><table>
            <thead><tr><th>Who</th><th>What they can see</th></tr></thead>
            <tbody>{SEES.map(([w, s]) => <tr key={w}><td>{w}</td><td>{s}</td></tr>)}</tbody>
          </table></div>
          <p className="fine">That is why "no server reading along" is about content. Connection details such as your address can be seen by the helpers above; you can run your own, use none on a local network, or hide your address with the IP options.</p>
        </div>
      </section>
      <section className="page-sec">
        <div className="wrap">
          <h2 className="group-h">Using more than one device</h2>
          <ol className="longsteps">
            <li><b>1</b><div><h3>The new device shows a QR code</h3><p>It has no account yet, so it asks to join.</p></div></li>
            <li><b>2</b><div><h3>Your phone scans it</h3><p>The phone holds the account key and shows six digits.</p></div></li>
            <li><b>3</b><div><h3>You type the six digits on the new device</h3><p>It checks them (three tries) before accepting. Someone who only photographed the QR code gets nowhere.</p></div></li>
          </ol>
        </div>
      </section>
      <section className="page-sec">
        <div className="wrap">
          <h2 className="group-h">Groups, in plain words</h2>
          <p className="sub">A group is a set of people who each link directly to each other. Add Ben, Ben adds Cat, and you and Cat connect automatically without sharing a code. Up to 10 people. Someone added by a member you trust shows as "Introduced by" that member, so use safety numbers for anyone who matters. Removing a member stops new messages reaching them but cannot take back what they already have. New members see only messages sent after they joined.</p>
        </div>
      </section>
      <section className="page-sec">
        <div className="wrap">
          <h2 className="group-h">Backups and recovery</h2>
          <ol className="longsteps">
            <li><b>1</b><div><h3>Your recovery phrase is the account</h3><p>It restores your identity on any device. It does not contain your chats.</p></div></li>
            <li><b>2</b><div><h3>Backups hold the chats</h3><p>Make a backup protected by a passphrase (12 characters minimum). Files and old groups are included. Automatic backups are available on Windows.</p></div></li>
            <li><b>3</b><div><h3>Restore on a new device</h3><p>Enter the 12 words, then open the backup with its passphrase. Friends see the same keys as before, so nothing needs re-verifying.</p></div></li>
          </ol>
        </div>
      </section>
      <section className="page-sec">
        <div className="wrap">
          <h2 className="group-h">When something goes wrong</h2>
          <div className="fgrid">
            <article className="fcard plain"><h3>A message is stuck</h3><p>Open Delivery details on it to see each device's state and the route. "Waiting" means that device has not answered yet.</p></article>
            <article className="fcard plain"><h3>You are not receiving</h3><p>Settings shows a plain-language reason (no connection, battery limits, notifications off, quiet hours, muted chats).</p></article>
            <article className="fcard plain"><h3>Two phones on mobile data cannot connect</h3><p>Many carriers share addresses between customers. A TURN relay fixes it; the repository has a ready kit, and the app can deliver via relays meanwhile.</p></article>
            <article className="fcard plain"><h3>You lost a device</h3><p>Remove it from another device. It takes effect in 24 hours, or at once with the recovery phrase.</p></article>
          </div>
        </div>
      </section>
      <Next Go={Go} items={[['features', 'Every feature'], ['faq', 'Questions'], ['download', 'Download']]} />
    </>
  );
}

export function ScreensPage({ Go, asset }) {
  return (
    <>
      <Gallery asset={asset} level="h1" />
      <section className="page-sec">
        <div className="wrap">
          <h2 className="group-h">What each screen shows</h2>
          <div className="fgrid">
            {SHOTS.map((s) => (<article className="fcard plain" key={s.src}><h3>{s.title}</h3><p>{s.note}</p></article>))}
          </div>
        </div>
      </section>
      <Next Go={Go} items={[['features', 'Every feature'], ['download', 'Download']]} />
    </>
  );
}

const COMPARE_MORE = [
  ['Platforms', 'Windows, Android', 'Android, iPhone, desktop', 'Android, iPhone, desktop', 'Android'],
  ['Source code', 'Source-available (PC-NC licence)', 'Open source', 'Open source', 'Open source'],
  ['Cost', 'Free', 'Free', 'Free', 'Free'],
  ['Post-quantum key exchange', 'Yes (ML-KEM-768 with P-256)', 'Yes (recent versions)', 'No', 'No'],
];
const WEAKER = [
  'It has not been independently audited yet, and Signal, Session and Briar have.',
  'It is young and maintained by one person.',
  'Only Windows and Android today.',
  'Far fewer people use it, so there is a smaller network of friends to reach.',
  'The Bluetooth mesh is experimental and tested in simulation only.',
  'Direct links reveal connection details to relays, STUN and TURN servers (never content).',
];

export function ComparePage({ Go }) {
  return (
    <>
      <Head eyebrow="Compare" title="PrivCord next to Signal, Session and Briar" lead="A good-faith summary. It can go out of date, so check each project for its current state." />
      <section className="page-sec">
        <div className="wrap">
          <div className="tablewrap"><table>
            <thead><tr><th></th><th>PrivCord</th><th>Signal</th><th>Session</th><th>Briar</th></tr></thead>
            <tbody>{COMPARE.map((r) => <tr key={r[0]}>{r.map((c, i) => <td key={i}>{c}</td>)}</tr>)}</tbody>
          </table></div>
        </div>
      </section>
      <section className="page-sec">
        <div className="wrap">
          <h2 className="group-h">More differences</h2>
          <div className="tablewrap"><table>
            <thead><tr><th></th><th>PrivCord</th><th>Signal</th><th>Session</th><th>Briar</th></tr></thead>
            <tbody>{COMPARE_MORE.map((r) => <tr key={r[0]}>{r.map((c, i) => <td key={i}>{c}</td>)}</tr>)}</tbody>
          </table></div>
          <p className="fine">Reading the table: "no server" for PrivCord means no company server stores or reads your messages. Optional relays, STUN and TURN still exist and can see connection details. Check each project for its current state.</p>
        </div>
      </section>
      <section className="page-sec">
        <div className="wrap">
          <h2 className="group-h">Where PrivCord is weaker</h2>
          <ul className="plainlist">{WEAKER.map((w) => <li key={w}>{w}</li>)}</ul>
          <p className="sub">If you need a battle-tested, audited messenger today, use one of the audited ones. PrivCord is for people who want to try a different design: no phone number, device-to-device links, and an offline mesh.</p>
        </div>
      </section>
      <Next Go={Go} items={[['how', 'How it works'], ['faq', 'Questions'], ['about', 'About']]} />
    </>
  );
}

const MORE_FAQ = [
  ['How do I add a friend?', 'Open Add friend. Show your QR code or read out the six words, and they scan or type them. You both tap Add. Both of you then have each other\'s keys pinned.'],
  ['What are the six words?', 'A short-lived meeting point, not your identity. Both phones turn them into a mailbox and a key so one can pass the other an encrypted contact card. They stop working after 15 minutes or when the app closes, and you confirm who answered with the safety numbers.'],
  ['What are safety numbers?', 'A short code both of you can compare (in person or by voice) to be sure you are talking to the right person and not an impostor. Use them for anyone who matters.'],
  ['What happens to messages when my friend is offline?', 'They wait on your device and send when you reconnect. Nothing is stored for you on a server. With "Deliver via relays" on, sealed copies can wait at a relay instead.'],
  ['Can I use it on my phone and my PC?', 'Yes. Link a new device: it shows a QR code, your phone scans it, and you type six digits on the new device. The account key never leaves your first device.'],
  ['What is the 12-word recovery phrase?', 'It is your account. The same words restore you on a new device. They are never stored anywhere else, so write them down and keep them safe. Anyone with them can be you.'],
  ['What if I forget my recovery phrase and lose my device?', 'Then the account cannot be recovered. There is no server and no reset, which is also why nobody else can take it over.'],
  ['What do backups contain, and who can open them?', 'Your chats, files and old groups, encrypted with a key made from your passphrase (Argon2id, 12 characters minimum). Without the passphrase they cannot be opened.'],
  ['How big can files be?', 'Up to 50 MB. They go straight from device to device, so both need a live link; a file for someone offline waits and sends when they reconnect.'],
  ['How many people can be in a group?', 'Up to 10. Every member links directly to every other, so 10 people means 9 links per device.'],
  ['Does it cost anything?', 'No. It is free. The source is public under the PC-NC licence, which does not allow commercial use without permission.'],
  ['Can I stop people messaging me?', 'Yes. "Stay group-only with people I meet in groups" blocks new direct chats with people you only know through a group. It is a courtesy between honest apps, not a guarantee against a modified one.'],
  ['Can I hide my IP address from friends?', 'Yes. "Hide my IP from friends" sends links through a relay only, and Tor mode routes through Tor. Both are slower. Relays you use can still see your address.'],
  ['Can I block screenshots?', 'On a device, yes: it blocks ordinary screenshots and screen recording. It cannot stop a camera pointed at the screen.'],
  ['What does the app ask permission for?', 'The camera (to scan QR codes), Bluetooth (for nearby mode, Android) and notifications. It does not ask for your contacts or phone number.'],
  ['Why is Windows warning me about the installer?', 'The installer is not code-signed yet, so Windows SmartScreen warns about unknown publishers. Choose More info, then Run anyway, if you downloaded it from the releases page.'],
  ['Where is my data stored?', 'On your device, encrypted. There is no PrivCord account or cloud copy. Your OS backups, if you have them on, may still keep copies.'],
  ['How do I delete everything?', 'Use the wipe option in Settings. It clears this device only; it cannot erase what friends already received.'],
  ['Are there ads or trackers?', 'No. The app collects nothing, and this website has no analytics or cookies.'],
  ['How do I report a bug or a security problem?', 'Bugs go in GitHub issues. For security problems, follow the security policy in the repository and report privately.'],
];

export function FaqPage({ Go }) {
  const all = [...FAQ, ...MORE_FAQ];
  return (
    <>
      <Head eyebrow="FAQ" title="Questions people ask" lead="Straight answers, including the uncomfortable ones." />
      <section className="page-sec">
        <div className="wrap">
          <div className="faq" style={{ marginTop: 0 }}>
            {all.map(([q, a]) => (<details key={q}><summary>{q}</summary><p>{a}</p></details>))}
          </div>
        </div>
      </section>
      <Next Go={Go} items={[['features', 'Every feature'], ['compare', 'Compare'], ['about', 'About']]} />
    </>
  );
}

export function DownloadPage({ Go, repo, reduce, dlRef, dlOn }) {
  const link = (label, file) => (
    <Magnet padding={60} magnetStrength={3} disabled={reduce}>
      <StarBorder as="a" href={`${repo}/releases/download/${file}`} color={YELLOW} trailColor="#ffffff" backgroundColor={YELLOW} textColor={INK} borderColor="rgba(0,0,0,0)" duration={5} radius={10} className="cta-star">{label}</StarBorder>
    </Magnet>
  );
  return (
    <>
      <section id="download" className="download page-dl" ref={dlRef}>
        {!reduce && <div className="dl-bg" aria-hidden="true"><DotGrid dotSize={4} gap={22} baseColor="#2b2b2b" activeColor={YELLOW} proximity={150} shockRadius={340} shockStrength={5} glow={0} paused={!dlOn} /></div>}
        <div className="wrap">
          <p className="eyebrow">Download</p>
          <h1>Get PrivCord</h1>
          <p className="sub">Free. Pick your platform.</p>
          <div className="dl">
            <div className="dl-row"><div><h3><Icon d={P.mon} />Windows</h3><p>Installer for Windows 10 and 11 (64-bit). Downloads the file directly.</p></div>{link('Download for Windows', 'Windows/PrivCord-setup.exe')}</div>
            <div className="dl-row"><div><h3><Icon d={P.ph} />Android</h3><p>One APK for Android phones. Downloads the file directly.</p></div>{link('Download for Android', 'Android/PrivCord.apk')}</div>
            <div className="dl-row"><div><h3><Icon d={P.mon} />Server (test build)</h3><p>Run your own relay, Nostr and TURN server, with a web admin panel. For people who host: a command-line program, not an app. Pick your system.</p></div>
              <div className="dl-server">
                <a className="btn primary small" href={`${repo}/releases/download/Server/privcord-server-linux-x86_64`}>Linux</a>
                <a className="btn primary small" href={`${repo}/releases/download/Server/privcord-server-windows-x86_64.exe`}>Windows</a>
                <a className="btn primary small" href={`${repo}/releases/download/Server/privcord-server-macos-arm64`}>macOS</a>
              </div></div>
          </div>
        </div>
      </section>
      <section className="page-sec">
        <div className="wrap">
          <h2 className="group-h">Install notes</h2>
          <div className="fgrid">
            <article className="fcard plain"><h3>Windows</h3><p>The installer is not code-signed yet, so Windows SmartScreen will warn you. Choose <em>More info</em>, then <em>Run anyway</em>.</p></article>
            <article className="fcard plain"><h3>Android</h3><p>You need to allow installing from the app or browser you downloaded it with. Pick the arm64 file unless your phone is very old.</p></article>
            <article className="fcard plain"><h3>First run</h3><p>The app makes your 12 recovery words. Write them down before you continue: they are the only way back to your account.</p></article>
            <article className="fcard plain"><h3>Requirements</h3><p>Windows 10 or 11. Android phones: arm64 for almost every phone, armeabi-v7a only for old 32-bit ones. The app asks for the camera (QR codes), Bluetooth (nearby mode, Android) and notifications, and never for your contacts.</p></article>
            <article className="fcard plain"><h3>Server</h3><p>Run <code>privcord-server init</code>, then <code>privcord-server doctor</code> to check your network, then <code>privcord-server serve</code>; the admin panel is at <code>/admin</code>. It is a test build and not independently audited. Read the <a href={`${repo}/blob/main/crates/privcord-server/README.md`}>server guide</a> first. Whoever runs a server is responsible for it.</p></article>
            <article className="fcard plain"><h3>Where files come from</h3><p>Only from the GitHub releases page of the project. We do not distribute it anywhere else, so treat other download sites with suspicion.</p></article>
            <article className="fcard plain"><h3>Before you rely on it</h3><p>PrivCord has not been independently audited. Read the <a href={`${repo}/blob/main/docs/KNOWN_LIMITATIONS.md`}>known limitations</a> first.</p></article>
          </div>
        </div>
      </section>
      <Next Go={Go} items={[['features', 'Every feature'], ['how', 'How it works'], ['faq', 'Questions']]} />
    </>
  );
}

// Edit these lines to change what the About page says.
const ABOUT = {
  people: [
    ['GitHub: raiden6767', 'https://github.com/raiden6767'],
    ['GitHub: anandks2006', 'https://github.com/anandks2006'],
    ['X: @AnandKrishnaK1', 'https://x.com/AnandKrishnaK1'],
  ],
};

export function AboutPage({ Go, repo }) {
  return (
    <>
      <Head eyebrow="About" title="The people behind PrivCord" lead="PrivCord is maintained by two people, in the open. There is no company behind it, no investors, and no data collected about you." />
      <section className="page-sec">
        <div className="wrap">
          <div className="fgrid">
            <article className="fcard plain">
              <h3>Maintainers</h3>
              <ul className="plainlist links">
                {ABOUT.people.map(([t, h]) => <li key={h}><a href={h}>{t}</a></li>)}
                <li><a href={repo}>The PrivCord source on GitHub</a></li>
              </ul>
            </article>
            <article className="fcard plain">
              <h3>Reaching us</h3>
              <p>Bugs and ideas go in <a href={`${repo}/issues`}>GitHub issues</a>. Security problems: read the <a href={`${repo}/blob/main/SECURITY.md`}>security policy</a> first and report privately. Licensing questions: message the X account above.</p>
            </article>
            <article className="fcard plain">
              <h3>Licence</h3>
              <p>The source is public and free for personal, educational and research use under the PC-NC licence. Commercial use needs written permission. That makes it source-available rather than open source in the OSI sense, and the licence text is in the repository.</p>
            </article>
            <article className="fcard plain">
              <h3>What we promise, and what we don't</h3>
              <p>We promise to say plainly what PrivCord does not do: it has not been independently audited, the Bluetooth mesh is experimental, and relays can see connection details. The <a href={`${repo}/blob/main/docs/KNOWN_LIMITATIONS.md`}>known limitations</a> are kept up to date. We don't promise it is safe for someone whose life depends on their messages.</p>
            </article>
            <article className="fcard plain">
              <h3>Privacy of this website</h3>
              <p>This site is plain static pages. It has no analytics, no cookies and no sign-up. Downloads come from the GitHub releases page.</p>
            </article>
            <article className="fcard plain">
              <h3>Want to help?</h3>
              <p>Test it on real phones and PCs, read the code, or review the protocol notes in the repository. Independent eyes are the thing the project needs most.</p>
            </article>
          </div>
        </div>
      </section>
      <Next Go={Go} items={[['features', 'Every feature'], ['compare', 'Compare'], ['download', 'Download']]} />
    </>
  );
}

const DONATE_URL = 'https://paypal.me/raiden43';

export function DonatePage({ Go }) {
  return (
    <>
      <Head eyebrow="Donate" title="Support PrivCord" lead="PrivCord is free, has no ads and collects nothing. If it is useful to you, you can chip in." />
      <section className="page-sec">
        <div className="wrap">
          <div className="fgrid">
            <article className="fcard plain">
              <h3>Donate with PayPal</h3>
              <p>Any amount helps. You pay on PayPal's own page, so PrivCord never sees your card or account details.</p>
              <p className="donate-cta"><a className="btn primary" href={DONATE_URL} target="_blank" rel="noopener noreferrer">Donate on PayPal</a></p>
              <p className="fine">paypal.me/raiden43</p>
            </article>
            <article className="fcard plain">
              <h3>What it supports</h3>
              <p>Time spent building and fixing PrivCord, and keeping the project free of ads, trackers and accounts. A donation does not unlock anything: everyone gets the same app.</p>
            </article>
            <article className="fcard plain">
              <h3>Prefer to help another way?</h3>
              <p>Test it on real phones and PCs, report bugs, or review the code. Independent eyes are the most useful thing right now. See <Go to="about">About</Go> for where to find us.</p>
            </article>
            <article className="fcard plain">
              <h3>Good to know</h3>
              <p>Donations are voluntary. PrivCord is a small independent project, not a registered charity, so a donation is not tax-deductible. It does not make PrivCord more secure by itself: it has not been independently audited yet.</p>
            </article>
          </div>
        </div>
      </section>
      <Next Go={Go} items={[['download', 'Download'], ['about', 'About'], ['faq', 'Questions']]} />
    </>
  );
}
