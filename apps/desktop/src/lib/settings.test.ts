import { describe, expect, it } from 'vitest';
import { parseSettings } from './settings';

describe('parseSettings', () => {
  it('restores persisted values', () => {
    expect(
      parseSettings(
        JSON.stringify({
          theme: 'dark',
          accent: '#0d9488',
          notificationsEnabled: false,
          notificationPreviews: false,
          displayName: '  Sam  ',
        }),
      ),
    ).toMatchObject({
      theme: 'dark',
      accent: '#0d9488',
      notificationsEnabled: false,
      notificationPreviews: false,
      displayName: 'Sam',
    });
  });

  it('falls back to defaults for missing or malformed storage', () => {
    expect(parseSettings(JSON.stringify({ theme: 'light' })).displayName).toBe('');
    expect(parseSettings('{not json').theme).toBe('system');
    expect(parseSettings(null).accent).toBe('#7a3e73');
  });

  it('shows the unread count unless it was turned off', () => {
    expect(parseSettings(null).showUnreadCount).toBe(true);
    expect(parseSettings(JSON.stringify({ showUnreadCount: false })).showUnreadCount).toBe(false);
    expect(parseSettings(JSON.stringify({ showUnreadCount: 'no' })).showUnreadCount).toBe(true);
  });

  it('starts in the simple view and remembers the Advanced switch', () => {
    expect(parseSettings(null).advanced).toBe(false);
    expect(parseSettings(JSON.stringify({ advanced: true })).advanced).toBe(true);
    expect(parseSettings(JSON.stringify({ advanced: 'yes' })).advanced).toBe(false);
  });

  it('rejects an invalid accent color', () => {
    expect(parseSettings(JSON.stringify({ accent: 'red' })).accent).toBe('#7a3e73');
  });
});
