import Gallery from './Gallery.jsx';
import StepStack from './StepStack.jsx';
import { Icon, P, FEATURES, STEPS, FAQ, COMPARE } from './data.jsx';
import { ArchitecturePage } from './Architecture.jsx';
import { ChangelogPage } from './Changelog.jsx';
import { FeaturesPage, HowPage, ScreensPage, ComparePage, FaqPage, DownloadPage, AboutPage, DonatePage } from './Pages.jsx';
import { createContext, useContext, useEffect, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
import DotGrid from './rb/DotGrid.jsx';
import SplitText from './rb/SplitText.jsx';
import ShinyText from './rb/ShinyText.jsx';
import ScrollVelocity from './rb/ScrollVelocity.jsx';
import ScrollReveal from './rb/ScrollReveal.jsx';
import ScrollFloat from './rb/ScrollFloat.jsx';
import SpotlightCard from './rb/SpotlightCard.jsx';
import GlareHover from './rb/GlareHover.jsx';
import ClickSpark from './rb/ClickSpark.jsx';
import Magnet from './rb/Magnet.jsx';
import StarBorder from './rb/StarBorder.jsx';
import AnimatedContent from './rb/AnimatedContent.jsx';
import FadeContent from './rb/FadeContent.jsx';

const REPO = 'https://github.com/__REPO__';
const EXE = `${REPO}/releases/download/Windows/PrivCord-setup.exe`;
const APK = `${REPO}/releases/download/Android/PrivCord.apk`;


function useReducedMotion() {
  const [r, setR] = useState(() => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches);
  useEffect(() => {
    const m = matchMedia('(prefers-reduced-motion: reduce)');
    const f = () => setR(m.matches);
    m.addEventListener('change', f);
    return () => m.removeEventListener('change', f);
  }, []);
  return r;
}

/** Pauses a canvas background while it is off screen. */
function useOnScreen(ref, page) {
  const [on, setOn] = useState(true);
  useEffect(() => {
    if (!ref.current) return;
    const io = new IntersectionObserver(([e]) => setOn(e.isIntersecting), { rootMargin: '120px' });
    io.observe(ref.current);
    return () => io.disconnect();
  }, [ref, page]);
  return on;
}

const NAV = [['features', 'Features'], ['how', 'How it works'], ['architecture', 'Architecture'], ['screens', 'Screens'], ['compare', 'Compare'], ['changelog', 'Changelog'], ['faq', 'FAQ'], ['about', 'About'], ['donate', 'Donate']];
const TITLES = { home: 'PrivCord: private encrypted messenger with no phone number or account', features: 'Features: PrivCord', how: 'How it works: PrivCord', architecture: 'Architecture: how PrivCord is built', screens: 'Screens: PrivCord', compare: 'How PrivCord compares', faq: 'Questions people ask: PrivCord', changelog: 'Changelog: what changed in PrivCord', download: 'Get PrivCord for Windows and Android', about: 'About the maker: PrivCord', donate: 'Support PrivCord: donate' };
// Real addresses: /features/, /how/, ... Each is also a pre-built HTML file (scripts/pages.mjs), so a link, a refresh or a search
// engine lands on the right page; inside the site, clicks switch pages without reloading. ROOT is wherever the site lives
// (the domain root, or /PrivCord/ on a project page), worked out from the address we were opened at.
const PAGES = Object.keys(TITLES);
const locate = () => {
  const parts = location.pathname.split('/').filter(Boolean);
  const last = parts[parts.length - 1];
  const route = PAGES.includes(last) && last !== 'home' ? last : 'home';
  const rootParts = route === 'home' ? parts : parts.slice(0, -1);
  return { route, root: `/${rootParts.join('/')}${rootParts.length ? '/' : ''}` };
};
const { route: FIRST, root: ROOT } = locate();
const asset = (n) => `${ROOT}assets/${n}`;
const hrefOf = (to) => (to === 'home' ? ROOT : `${ROOT}${to}/`);
// Old hash links (#faq, #/faq) from before pages had addresses still land on the right page.
const hashPage = () => {
  const h = location.hash.replace(/^#\/?/, '');
  return h === 'top' ? 'home' : TITLES[h] ? h : null;
};
function usePage() {
  const [route, setRoute] = useState(() => hashPage() || FIRST);
  useEffect(() => {
    const fromHash = () => {
      const legacy = hashPage();
      if (legacy) history.replaceState(null, '', hrefOf(legacy));
    };
    fromHash();
    // Switching page cross-fades with the browser's View Transitions where they exist (the header stays put); other browsers
    // get the CSS entrance on .page. Reduced motion switches instantly.
    const on = () => {
      const next = locate().route;
      const animate = typeof document.startViewTransition === 'function' && !matchMedia('(prefers-reduced-motion: reduce)').matches;
      if (!animate) { setRoute(next); return; }
      document.documentElement.classList.add('vt');
      const t = document.startViewTransition(() => {
        flushSync(() => setRoute(next));
        window.scrollTo({ top: 0, behavior: 'instant' });
      });
      t.finished.finally(() => document.documentElement.classList.remove('vt'));
    };
    const onHash = () => { fromHash(); on(); };
    addEventListener('popstate', on);
    addEventListener('hashchange', onHash);
    return () => { removeEventListener('popstate', on); removeEventListener('hashchange', onHash); };
  }, []);
  return route;
}
/** An ordinary link to a real address; a plain left click switches page in place. */
function Go({ to, children, ...p }) {
  const click = (e) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    if (hrefOf(to) !== location.pathname) history.pushState(null, '', hrefOf(to));
    dispatchEvent(new PopStateEvent('popstate'));
  };
  return <a href={hrefOf(to)} onClick={click} {...p}>{children}</a>;
}

const ReduceCtx = createContext(false);
// Wrappers that render plain content when the visitor asked for reduced motion. They live outside App so they keep their identity.
function Anim({ children, ...p }) {
  return useContext(ReduceCtx) ? <div className={p.className}>{children}</div> : <AnimatedContent {...p}>{children}</AnimatedContent>;
}
function Fade({ children, ...p }) {
  return useContext(ReduceCtx) ? <div className={p.className}>{children}</div> : <FadeContent {...p}>{children}</FadeContent>;
}
const INK = '#0a0a0a';
const YELLOW = '#ffe500';

export default function App() {
  const reduce = useReducedMotion();
  const route = usePage();
  const mainRef = useRef(null);
  const first = useRef(true);
  useEffect(() => {
    document.title = TITLES[route];
    if (first.current) { first.current = false; return; }
    window.scrollTo({ top: 0, behavior: 'instant' });
    mainRef.current?.focus({ preventScroll: true });
  }, [route]);
  const heroRef = useRef(null);
  const dlRef = useRef(null);
  const heroOn = useOnScreen(heroRef, route);
  const dlOn = useOnScreen(dlRef, route);
  const split = (text, extra = {}) =>
    reduce ? text : <SplitText tag="span" className="line" text={text} splitType="chars" delay={22} duration={0.9} textAlign="left" from={{ opacity: 0, y: 60 }} to={{ opacity: 1, y: 0 }} {...extra} />;

  return (
    <ReduceCtx.Provider value={reduce}>
      <ClickSpark sparkColor={YELLOW} sparkCount={10} sparkRadius={26} duration={600} disabled={reduce}>
        <div className="progress" aria-hidden="true" />
        <a className="skip" href="#main">Skip to content</a>

        <header className="top">
          <div className="wrap">
            <Go className="brand" to="home"><img src={asset('icon.png')} alt="" width="30" height="30" />PrivCord</Go>
            <nav className="links" aria-label="Main">
              {NAV.map(([to, t]) => <Go key={to} to={to} aria-current={route === to ? 'page' : undefined}>{t}</Go>)}
              <Go className="btn primary small" to="download">Download</Go>
            </nav>
            <details className="mnav" key={route}>
              <summary aria-label="Menu"><span aria-hidden="true">Menu</span></summary>
              <div className="mnav-panel">
                {[['home', 'Home'], ...NAV, ['download', 'Download']].map(([to, t]) => <Go key={to} to={to} aria-current={route === to ? 'page' : undefined}>{t}</Go>)}
              </div>
            </details>
          </div>
        </header>

        <main id="main" tabIndex={-1} ref={mainRef}>
          <div className="page" key={route}>
          {route === 'home' && (
            <>
{/* ---------- hero: ink, with a springy dot grid the cursor flicks around ---------- */}
          <section className="hero" id="top" ref={heroRef}>
            {!reduce && (
              <div className="hero-bg" aria-hidden="true">
                <DotGrid dotSize={4} gap={22} baseColor="#2b2b2b" activeColor={YELLOW} proximity={150} shockRadius={340} shockStrength={5} glow={0} paused={!heroOn} />
              </div>
            )}
            <div className="wrap hero-grid">
              <div className="hero-copy">
                <h1 className="display">
                  <span className="row-line">{split('Chat privately.')}</span>
                  <span className="row-line"><mark>{split('No number, no account,')}</mark></span>
                  <span className="row-line">{split('no server reading along.', { delay: 18 })}</span>
                </h1>
                <p className="lead">PrivCord is a peer-to-peer messenger for Windows and Android. Chats go straight from your device to your friend's over an encrypted link, you add people with a QR code or six words, and it can keep going over Bluetooth with no internet.</p>
                <div className="cta">
                  <Magnet padding={80} magnetStrength={3} disabled={reduce}>
                    <StarBorder as={Go} to="download" color={YELLOW} trailColor="#ffffff" backgroundColor={YELLOW} textColor={INK} borderColor="rgba(0,0,0,0)" duration={5} radius={10} className="cta-star">Download for Windows or Android</StarBorder>
                  </Magnet>
                  <a className="btn ghost" href={REPO}>View the source</a>
                </div>
                <p className="note">
                  {reduce ? 'End-to-end encrypted · source-available · not yet independently audited' : <ShinyText text="End-to-end encrypted · source-available · not yet independently audited" color="#8f8b82" shineColor="#ffffff" speed={3.5} />}
                </p>
              </div>
            </div>
          </section>

{/* ---------- yellow marquee that speeds up and skews with scroll ---------- */}
          <section className="band" aria-label="What PrivCord does not ask of you">
            {reduce ? (
              <ul className="band-static"><li>No phone number</li><li>No account</li><li>No server reading along</li></ul>
            ) : (
              <ScrollVelocity texts={['NO PHONE NUMBER • NO ACCOUNT •', 'NO SERVER READING ALONG • NO TRACKING •']} velocity={60} className="vel" numCopies={5} skewWithVelocity={4} outlineAlternate />
            )}
          </section>

{/* ---------- statement on paper ---------- */}
          <section className="statement">
            <div className="wrap narrow">
              {reduce ? (
                <p className="big">Your messages go from your device straight to your friend's. There is no company in the middle, no phone number to hand over, and no account to lose.</p>
              ) : (
                <ScrollReveal baseOpacity={0.14} enableBlur blurStrength={6} baseRotation={2} textClassName="big" containerClassName="statement-box">
                  Your messages go from your device straight to your friend's. There is no company in the middle, no phone number to hand over, and no account to lose.
                </ScrollReveal>
              )}
            </div>
          </section>

{/* ---------- features ---------- */}
          <section id="features" className="features">
            <div className="wrap">
              <div className="sec-head">
                {reduce ? <h2>Everything a chat app needs, nothing that watches you</h2> : <ScrollFloat containerClassName="sf" textClassName="sf-text" splitBy="words" stagger={0.06}>Everything a chat app needs, nothing that watches you</ScrollFloat>}
                <p className="sub">Built for people who want private conversations without handing over an identity.</p>
              </div>
              <div className="bento">
                {FEATURES.map((f, n) => (
                  <Anim key={f.t} className={`bcell ${f.cls}`} distance={70} direction="vertical" duration={0.9} delay={(n % 3) * 0.08} scale={0.96} blur={6}>
                    <SpotlightCard className="fcard" spotlightColor="rgba(255, 229, 0, 0.2)" spotlightSize={260} intensity={0.25} theme="dark">
                      <h3><Icon d={P[f.i]} />{f.t}{f.tag ? <span className="tag">{f.tag}</span> : null}</h3>
                      <p>{f.p}</p>
                    </SpotlightCard>
                  </Anim>
                ))}
              </div>
              <p className="more-row"><Go className="more" to="features">See every feature in detail →</Go></p>
            </div>
          </section>

{/* ---------- how it works: sticky stacking cards on paper ---------- */}
          <section id="how" className="how">
            <div className="wrap how-grid">
              <div className="how-lhs">
                <h2>Up and running in three steps</h2>
                <p className="sub">No sign-up form. Your identity is made on your device.</p>
              </div>
              <StepStack
                steps={STEPS}
                intro={<><h2>Up and running in three steps</h2><p className="sub">No sign-up form. Your identity is made on your device.</p></>}
                more={<Go className="more" to="how">How it works, in detail →</Go>}
              />
            </div>
          </section>

{/* ---------- screens: a rail of real screenshots and a lightbox ---------- */}

{/* ---------- compare (paper) ---------- */}
          <section id="compare" className="compare">
            <div className="wrap">
              <h2>How PrivCord compares</h2>
              <p className="sub">A short, good-faith summary. It can go out of date, so check each project for its current state.</p>
              <Fade blur duration={1} className="tablewrap-wrap">
                <div className="tablewrap">
                  <table>
                    <thead><tr><th></th><th>PrivCord</th><th>Signal</th><th>Session</th><th>Briar</th></tr></thead>
                    <tbody>{COMPARE.map((r) => <tr key={r[0]}>{r.map((c, i) => <td key={i}>{c}</td>)}</tr>)}</tbody>
                  </table>
                </div>
              </Fade>
              <p className="fine">PrivCord is a young project by one maintainer. If you need a battle-tested, audited messenger today, use one of the audited ones.</p>
              <p className="more-row"><Go className="more" to="compare">The full comparison →</Go></p>
            </div>
          </section>

{/* ---------- faq ---------- */}
          <section id="faq" className="faq-sec">
            <div className="wrap">
              <h2>Questions people ask</h2>
              <div className="faq">
                {FAQ.slice(0, 5).map(([q, a], n) => (
                  <Anim key={q} distance={30} duration={0.6} delay={Math.min(n, 3) * 0.05}>
                    <details><summary>{q}</summary><p>{a}</p></details>
                  </Anim>
                ))}
              </div>
              <p className="more-row"><Go className="more" to="faq">All questions →</Go></p>
            </div>
          </section>

{/* ---------- download: yellow ---------- */}
          <section id="download" className="download" ref={dlRef}>
            {!reduce && <div className="dl-bg" aria-hidden="true"><DotGrid dotSize={4} gap={22} baseColor="#2b2b2b" activeColor={YELLOW} proximity={150} shockRadius={340} shockStrength={5} glow={0} paused={!dlOn} /></div>}
            <div className="wrap">
              <h2>Get PrivCord</h2>
              <p className="sub">Free. Pick your platform.</p>
              <div className="dl">
                <div className="dl-row">
                  <div><h3><Icon d={P.mon} />Windows</h3><p>Installer for Windows 10 and 11 (64-bit). Downloads the file directly.</p></div>
                  <Magnet padding={60} magnetStrength={3} disabled={reduce}><StarBorder as="a" href={EXE} color={YELLOW} trailColor="#ffffff" backgroundColor={YELLOW} textColor={INK} borderColor="rgba(0,0,0,0)" duration={5} radius={10} className="cta-star">Download the installer</StarBorder></Magnet>
                </div>
                <div className="dl-row">
                  <div><h3><Icon d={P.ph} />Android</h3><p>One APK for Android phones. Downloads the file directly.</p></div>
                  <Magnet padding={60} magnetStrength={3} disabled={reduce}><StarBorder as="a" href={APK} color={YELLOW} trailColor="#ffffff" backgroundColor={YELLOW} textColor={INK} borderColor="rgba(0,0,0,0)" duration={5} radius={10} className="cta-star">Download the APK</StarBorder></Magnet>
                </div>
                <div className="dl-row">
                  <div><h3><Icon d={P.mon} />Server (test build)</h3><p>Host your own relay, Nostr and TURN server. For people who run servers.</p></div>
                  <div className="dl-server">
                    <a className="btn primary small" href={`${REPO}/releases/download/Server/privcord-server-linux-x86_64`}>Linux</a>
                    <a className="btn primary small" href={`${REPO}/releases/download/Server/privcord-server-windows-x86_64.exe`}>Windows</a>
                    <a className="btn primary small" href={`${REPO}/releases/download/Server/privcord-server-macos-arm64`}>macOS</a>
                  </div>
                </div>
              </div>
              <p className="warn"><strong>Heads up:</strong> the Windows installer is not code-signed yet, so Windows SmartScreen will warn you: choose <em>More info</em>, then <em>Run anyway</em>. On Android you need to allow installing apps from this source. Windows will also ask to allow the firewall the first time, which lets PrivCord find friends on your local network.</p>
              <p className="more-row"><Go className="more" to="download">Install notes →</Go></p>
            </div>
          </section>
            </>
          )}
          {route === 'features' && <FeaturesPage Go={Go} />}
          {route === 'how' && <HowPage Go={Go} />}
          {route === 'architecture' && <ArchitecturePage Go={Go} repo={REPO} />}
          {route === 'screens' && <ScreensPage Go={Go} asset={asset} />}
          {route === 'compare' && <ComparePage Go={Go} />}
          {route === 'faq' && <FaqPage Go={Go} />}
          {route === 'changelog' && <ChangelogPage Go={Go} repo={REPO} />}
          {route === 'download' && <DownloadPage Go={Go} repo={REPO} reduce={reduce} dlRef={dlRef} dlOn={dlOn} />}
          {route === 'about' && <AboutPage Go={Go} repo={REPO} />}
          {route === 'donate' && <DonatePage Go={Go} />}
          </div>
        </main>

        <footer>
          <div className="wrap foot">
            <div>© 2026 PrivCord · <a href={`${REPO}/blob/main/LICENSE`}>PC-NC licence (source-available)</a> · Fonts: Bricolage Grotesque, Figtree (OFL)</div>
            <nav aria-label="Footer">
              <a href={REPO}>GitHub</a>
              <a href={`${REPO}/blob/main/SECURITY.md`}>Security</a>
              <a href={`${REPO}/blob/main/docs/KNOWN_LIMITATIONS.md`}>Known limitations</a>
              <a href={`${REPO}/issues`}>Report an issue</a>
            </nav>
          </div>
        </footer>
      </ClickSpark>
    </ReduceCtx.Provider>
  );
}
