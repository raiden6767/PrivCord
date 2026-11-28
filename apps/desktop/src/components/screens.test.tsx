// @vitest-environment happy-dom
/**
 * The main screens rendered against an in-memory fake engine. These replace the UI flows of the old browser end-to-end suite
 * (which ran the TypeScript engine in real browsers); what the engine itself does is tested in crates/ and by
 * `hosted/e2e.test.ts` (the page's engine adapter against two real Rust engine processes).
 */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import { ChatLayout } from './ChatLayout';
import { AddFriendDialog } from './AddFriendDialog';
import { parseSettings } from '../lib/settings';
import { baseState, contact, fakeEngine, group, message, type FakeEngine } from '../test/fakeEngine';
import type { ChatState } from '../lib/chatTypes';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  document.body.replaceChildren();
});

const settings = { ...parseSettings(null), panicWipe: false };
const settle = () => act(async () => { await new Promise((r) => setTimeout(r, 0)); });

async function show(state: ChatState, opts: { panicWipe?: boolean; engine?: FakeEngine } = {}) {
  const engine = opts.engine ?? fakeEngine(state);
  const host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
  const r = root;
  const Screen = () => <ChatLayout state={engine.getState()} engine={engine} settings={{ ...settings, panicWipe: !!opts.panicWipe }} updateSettings={() => undefined} fatal={null} />;
  await act(async () => r.render(<Screen />));
  // Re-render on every state the engine publishes, as `useChatEngine` does.
  engine.subscribe(() => act(() => r.render(<Screen />)));
  await settle();
  return engine;
}

const q = <T extends Element = HTMLElement>(sel: string) => document.querySelector<T>(sel);
const qa = (sel: string) => Array.from(document.querySelectorAll<HTMLElement>(sel));
const click = (el: Element | null | undefined) => act(async () => { (el as HTMLElement).click(); });
const byLabel = (label: string) => q(`[aria-label="${label}"]`);
const chatRow = (name: string) => qa('.chat-row').find((r) => r.textContent?.includes(name));

/** Type into a React-controlled field the way a person does. */
function type(el: HTMLInputElement | HTMLTextAreaElement, value: string) {
  return act(async () => {
    const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, 'value')?.set?.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

describe('chat list and direct chat', () => {
  const ann = contact('u-ann', 'Ann');
  const bob = contact('u-bob', 'Bob', { unread: 2 });

  it('lists friends, opens a chat, sends text, and shows delivery ticks and incoming messages', async () => {
    const engine = await show(baseState({ contacts: [ann, bob], messages: { 'u-ann': [message('1', 'u-ann', 'hi from them', false)] } }));
    expect(qa('.chat-row').map((r) => r.querySelector('strong')?.textContent)).toEqual(['Ann', 'Bob']);
    expect(chatRow('Bob')?.querySelector('.unread-dot')?.textContent).toBe('2');

    await click(chatRow('Ann'));
    expect(engine.calls.openConversation).toHaveBeenCalledWith('u-ann');
    expect(q('.chat-header h2')?.textContent).toContain('Ann');
    expect(qa('.bubble-text').map((e) => e.textContent)).toEqual(['hi from them']);

    const box = byLabel('Message text') as HTMLTextAreaElement;
    await type(box, 'hello Ann');
    await click(byLabel('Send message'));
    expect(engine.calls.send).toHaveBeenCalledWith('u-ann', 'hello Ann', undefined);
    expect(box.value).toBe('');

    // The engine publishes our message as sent, then delivered, then read: the tick follows.
    const mine = message('2', 'u-ann', 'hello Ann', true);
    engine.push({ messages: { 'u-ann': [message('1', 'u-ann', 'hi from them', false), mine] } });
    await settle();
    const tick = () => q('.bubble.own .delivery-indicator')?.getAttribute('title') ?? '';
    expect(tick()).toMatch(/sent/i);
    engine.push({ messages: { 'u-ann': [message('2', 'u-ann', 'hello Ann', true, { tick: 'DELIVERED', deliveryState: 'DELIVERED', delivered: 1 })] } });
    await settle();
    expect(tick()).toMatch(/delivered/i);
    engine.push({ messages: { 'u-ann': [message('2', 'u-ann', 'hello Ann', true, { tick: 'READ', deliveryState: 'READ', delivered: 1 })] } });
    await settle();
    expect(tick()).toMatch(/read/i);
  });

  it('keeps what is typed when a send fails, and says why', async () => {
    const engine = fakeEngine(baseState({ contacts: [ann] }), { send: async () => ({ ok: false, error: 'Message is too long.' }) });
    await show(engine.getState(), { engine });
    await click(chatRow('Ann'));
    const box = byLabel('Message text') as HTMLTextAreaElement;
    await type(box, 'x');
    await click(byLabel('Send message'));
    expect(box.value).toBe('x');
    expect(document.body.textContent).toContain('Message is too long.');
  });

  it('replies to a message: the engine gets the id, and the quote shows on the bubble', async () => {
    const theirs = message('m1', 'u-ann', 'lunch at noon?', false);
    const engine = await show(baseState({ contacts: [ann], messages: { 'u-ann': [theirs] } }));
    await click(chatRow('Ann'));
    await click(byLabel('Reply to this message'));
    await type(byLabel('Message text') as HTMLTextAreaElement, 'yes');
    await click(byLabel('Send message'));
    expect(engine.calls.send).toHaveBeenCalledWith('u-ann', 'yes', 'm1');
    engine.push({ messages: { 'u-ann': [theirs, message('m2', 'u-ann', 'yes', true, { replyTo: { messageId: 'm1', senderId: 'u-ann', preview: 'lunch at noon?' } } as never)] } });
    await settle();
    expect(q('.bubble.own')?.textContent).toContain('lunch at noon?');
  });

  it('reactions and delete for everyone go to the engine; a deleted message becomes a tombstone', async () => {
    const theirs = message('m1', 'u-ann', 'secret', false);
    const engine = await show(baseState({ contacts: [ann], messages: { 'u-ann': [theirs] } }));
    await click(chatRow('Ann'));
    await click(byLabel('More actions'));
    await click(qa('button').find((b) => b.getAttribute('aria-label')?.startsWith('React ')));
    expect(engine.calls.react).toHaveBeenCalledWith('m1', expect.any(String));
    engine.push({ messages: { 'u-ann': [{ ...theirs, deleted: true, text: '' }] } });
    await settle();
    expect(q('.bubble-text.deleted')?.textContent).toBe('This message was deleted');
  });

  it('search finds messages from the engine and shows the chat they belong to', async () => {
    const engine = fakeEngine(baseState({ contacts: [ann] }), {
      search: () => [{ messageId: 'm9', convId: 'u-ann', convName: 'Ann', senderName: 'Ann', text: 'the picnic plan', at: 1 }],
    });
    await show(engine.getState(), { engine });
    await type(byLabel('Search messages') as HTMLInputElement, 'picnic');
    await settle();
    expect(q('.search-results')?.textContent).toContain('the picnic plan');
    expect(engine.calls.search).toHaveBeenCalled();
  });
});

describe('groups', () => {
  it('shows members and who is connected; the details screen counts toward the limit of 10', async () => {
    const g = group('g1', 'Trip', 10, 9);
    await show(baseState({ groups: [g], messages: { g1: [message('1', 'g1', 'welcome', false)] } }));
    await click(chatRow('Trip'));
    expect(q('.chat-header-identity p')?.textContent).toContain('10 members · 9 connected');
    await click(byLabel('More'));
    await click(qa('[role="menuitem"]').find((b) => /details|members/i.test(b.textContent ?? '')));
    expect(document.body.textContent).toContain('10 of 10 members');
  });

  it('a group message is sent to the group id', async () => {
    const engine = await show(baseState({ groups: [group('g1', 'Trip', 3, 2)] }));
    await click(chatRow('Trip'));
    await type(byLabel('Message text') as HTMLTextAreaElement, 'hello all');
    await click(byLabel('Send message'));
    expect(engine.calls.send).toHaveBeenCalledWith('g1', 'hello all', undefined);
  });

  it('a group you were removed from is read-only', async () => {
    await show(baseState({ groups: [group('g1', 'Trip', 3, 0, { status: 'removed' })] }));
    await click(chatRow('Trip'));
    expect(q('.chat-header-identity p')?.textContent).toContain('You were removed');
    expect(byLabel('Message text')).toBeNull();
  });
});

describe('open channels', () => {
  it('lists a joined channel and labels it public', async () => {
    const state = baseState({
      channels: [{ id: 'chan:general', name: 'general', unread: 0, conn: 'connected' }],
      messages: { 'chan:general': [message('c1', 'chan:general', 'hello', false, { senderName: 'Ann #1a2b' })] },
    });
    await show(state);
    await click(chatRow('general'));
    expect(q('.chat-header-identity p')?.textContent).toContain('Public channel');
    expect(document.body.textContent).toContain('Ann #1a2b');
  });
});

describe('add a friend', () => {
  async function dialog(engine: FakeEngine) {
    const host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
    const r = root;
    await act(async () => r.render(<AddFriendDialog open onClose={() => undefined} engine={engine} answer={null} onAdded={() => undefined} />));
    await settle();
  }

  it('shows six plain words to read out, and stops publishing them on close', async () => {
    const engine = fakeEngine(baseState());
    await dialog(engine);
    expect(document.body.textContent).toContain('appleriverstonecloudmaplequiet'); // one span per word
    act(() => root?.unmount());
    root = null;
    expect(engine.calls.closeInvite).toHaveBeenCalled();
  });

  it('wrong words fail with a clear message instead of adding anyone', async () => {
    const engine = fakeEngine(baseState(), { previewInvite: async () => { throw new Error('Those words did not match anyone.'); } });
    await dialog(engine);
    await type(byLabel("Friend's invite") as HTMLTextAreaElement, 'one two three four five six');
    await click(qa('button').find((b) => /^continue/i.test(b.textContent?.trim() ?? '')));
    expect(document.body.textContent).toContain('Those words did not match anyone.');
    expect(engine.calls.addContact).toBeUndefined();
  });

  it('a good invite shows who it is and the safety number before adding', async () => {
    const engine = fakeEngine(baseState(), { previewInvite: async () => ({ uid: 'u-new', name: 'Casey', safetyNumber: '12345 67890', already: false }) });
    await dialog(engine);
    await type(byLabel("Friend's invite") as HTMLTextAreaElement, 'privcord-invite-casey');
    await click(qa('button').find((b) => /^continue/i.test(b.textContent?.trim() ?? '')));
    expect(document.body.textContent).toContain('Casey');
    expect(document.body.textContent).toContain('12345 67890');
  });
});

describe('panic wipe', () => {
  const press = () => window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Backspace', ctrlKey: true, shiftKey: true }));

  it('does nothing while it is off', async () => {
    await show(baseState());
    act(() => { press(); press(); press(); });
    await settle();
    expect(document.body.textContent).not.toContain('Erase everything on this device');
  });

  it('three quick presses erase through the engine once it is on, and two do not', async () => {
    await show(baseState(), { panicWipe: true });
    act(() => { press(); press(); });
    await settle();
    expect(document.body.textContent).not.toContain('Erase everything on this device');
    act(() => { press(); });
    await settle();
    expect(document.body.textContent).toContain('Erase everything on this device');
  });
});
