import { describe, expect, it } from 'vitest';
import { MIN_PASSPHRASE, isWrongPassphrase, newPassphraseProblem, vaultErrorText } from './vault';

describe('a new vault passphrase', () => {
  it('needs the minimum length, counted in characters, and the same text twice', () => {
    expect(MIN_PASSPHRASE).toBe(8);
    expect(newPassphraseProblem('short', 'short')).toBe('vault.too_short');
    expect(newPassphraseProblem('', '')).toBe('vault.too_short');
    // Seven Arabic letters are seven characters, however many bytes.
    expect(newPassphraseProblem('كلمةسرق', 'كلمةسرق')).toBe('vault.too_short');
    expect(newPassphraseProblem('كلمة سر طويلة', 'كلمة سر طويلة')).toBeNull();
    expect(newPassphraseProblem('correct horse battery', 'correct horse batterY')).toBe('vault.mismatch');
    expect(newPassphraseProblem('correct horse battery', 'correct horse battery')).toBeNull();
  });
});

describe('a wrong passphrase', () => {
  it('is the service refusing the passphrase, not any invalid request', () => {
    expect(isWrongPassphrase({ kind: 'invalid', message: 'Wrong passphrase for this vault.' })).toBe(true);
    expect(isWrongPassphrase({ kind: 'invalid', message: "My Drive/Photos isn't empty." })).toBe(false);
    expect(isWrongPassphrase({ kind: 'locked', message: 'Wrong passphrase' })).toBe(false);
    expect(isWrongPassphrase(new Error('Wrong passphrase'))).toBe(false);
  });
});

describe('an error from adding a pair or a vault', () => {
  it("shows the service's own words when Drive couldn't be checked, not 'check the internet connection'", () => {
    // Before a new pair is saved the service checks whether its Drive folder is a vault; when Drive is slow
    // to answer the window said "Can't reach Google Drive. Check the internet connection." (2026-10).
    const slow = {
      kind: 'network',
      message:
        "Wasl couldn't check whether My Drive/Photos on Drive is an encrypted vault (Drive didn't answer in time). Try again when Drive can be reached.",
    };
    expect(vaultErrorText(slow)).toContain("Drive didn't answer in time");
    expect(vaultErrorText(slow)).not.toContain('internet connection');
    expect(vaultErrorText({ kind: 'invalid', message: "My Drive/Photos isn't empty." })).toContain("isn't empty");
    // Without words of its own, the usual sentence for the kind.
    expect(vaultErrorText({ kind: 'network', message: '' })).toContain('internet connection');
  });
});
