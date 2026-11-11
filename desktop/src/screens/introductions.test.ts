import { describe, expect, it } from 'vitest';
import { setLang } from '../i18n';
import type { DeviceInfo, OfferInfo, PairState } from '../types';
import { addedByName, isNew, offerablePairs, sharedWithText, waitingOffers } from './introductions';

const dev = (id: string, name: string, more: Partial<DeviceInfo> = {}): DeviceInfo => ({
  id,
  name,
  this_device: false,
  connected: false,
  path: null,
  added_at: 0,
  ...more,
});

const offer = (status: OfferInfo['status'], name = 'Camera'): OfferInfo => ({
  share: 'ab'.repeat(16),
  from_device: 'WASL-A',
  from_name: 'Phone',
  name,
  files: 3,
  bytes: 1000,
  first_seen: 0,
  status,
});

/** Placeholders arrive isolated (FSI…PDI) so names of either direction read right; compare without. */
const plain = (s: string | null) => s?.replace(/[⁦-⁩]/g, '') ?? null;

describe('introductions', () => {
  setLang('en');
  const me = dev('WASL-ME', 'Study PC', { this_device: true });
  const phone = dev('WASL-A', 'Phone');
  const tablet = dev('WASL-T', 'Tablet', { added_by: 'WASL-A', known: false });

  it('call a device new only when another added it and it is not kept here', () => {
    expect(isNew(tablet)).toBe(true);
    expect(isNew({ ...tablet, known: true })).toBe(false);
    // A service before introductions sends no `known`: nothing is new.
    expect(isNew(phone)).toBe(false);
    expect(isNew({ ...me, known: false })).toBe(false);
  });

  it('name who added a device, or its ID when this computer does not know it', () => {
    expect(addedByName(tablet, [me, phone, tablet])).toBe('Phone');
    expect(addedByName({ ...tablet, added_by: 'WASL-GONE' }, [me, tablet])).toBe('WASL-GONE');
    expect(addedByName(phone, [me, phone])).toBeNull();
  });

  it('show only the offers still waiting for an answer', () => {
    const list = [offer('new', 'Camera'), offer('declined', 'Old'), offer('accepted', 'Taken')];
    expect(waitingOffers(list).map((o) => o.name)).toEqual(['Camera']);
  });

  it('say who a pair is shared with, by name', () => {
    const all = [me, phone, tablet];
    expect(plain(sharedWithText(true, [], all))).toBe('Shared with all your devices');
    expect(plain(sharedWithText(false, ['WASL-A', 'WASL-T'], all))).toBe('Shared with Phone, Tablet');
    expect(plain(sharedWithText(true, ['WASL-T'], all))).toBe('Shared with all your devices, and offered to Tablet');
    expect(plain(sharedWithText(false, [], all))).toBe('Shared with no device yet');
    expect(plain(sharedWithText(false, ['WASL-GONE'], all))).toBe('Shared with WASL-GONE');
  });

  it('offer only pairs that sync with devices', () => {
    const pair = (id: string, route: string | undefined) => ({ pair: { id, name: id, route } }) as unknown as PairState;
    expect(offerablePairs([pair('a', 'drive'), pair('b', 'devices'), pair('c', 'drive_and_devices'), pair('d', undefined)]).map((s) => s.pair.id)).toEqual(['b', 'c']);
  });
});
