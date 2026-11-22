// @vitest-environment happy-dom
import { act, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { describe, expect, it } from 'vitest';
import { DigitBoxes } from './DigitBoxes';
import { Modal } from './Modal';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** Like the link dialog: a new onClose on every render, and the digit boxes appear after the dialog is open. */
function Harness({ onDone }: { onDone: (d: string) => void }) {
  const [shown, setShown] = useState(false);
  const [digits, setDigits] = useState('');
  return (
    <Modal open onClose={() => undefined} title="Link">
      <button type="button" id="show" onClick={() => setShown(true)}>show</button>
      {shown ? <DigitBoxes value={digits} onChange={setDigits} onComplete={onDone} label="Digits" /> : null}
    </Modal>
  );
}

describe('dialog focus while typing digits', () => {
  it('stays in the boxes and moves along; it does not jump back to the close button', async () => {
    const host = document.createElement('div');
    document.body.appendChild(host);
    const root = createRoot(host);
    let done = '';
    await act(async () => root.render(<Harness onDone={(d) => (done = d)} />));
    await act(async () => document.getElementById('show')?.click());
    const boxes = () => Array.from(document.querySelectorAll<HTMLInputElement>('.digit-box'));
    expect(document.activeElement).toBe(boxes()[0]);
    const type = async (el: HTMLInputElement, d: string) =>
      act(async () => {
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
        setter?.call(el, d);
        el.dispatchEvent(new Event('input', { bubbles: true }));
      });
    for (let i = 0; i < 6; i += 1) {
      await type(boxes()[i] as HTMLInputElement, String((i * 3 + 1) % 10));
      if (i < 5) expect(document.activeElement).toBe(boxes()[i + 1]);
    }
    expect(done).toBe('147036');
    expect(document.activeElement?.getAttribute('aria-label')).not.toBe('Close dialog');
    await act(async () => root.unmount());
  });
});
