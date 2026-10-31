/* Who a player can unsubscribe from in their deck completion alert settings
   (`/api/deck-subscriptions/{user}`), shared by the leaderboard dialog and /settings. */

export interface AlertSender {
  user: string;
  display: string;
}

/** `ReceivingSettings` in src/decks.rs, as far as choosing senders goes. */
export interface AlertSettings {
  senders: AlertSender[];
  unsubscribed_senders: string[];
  sharing_senders: string[];
  muted_senders?: string[];
}

/**
 * The senders to offer and those to show as unsubscribed. Saving replaces older
 * mutes with unsubscribes, so mutes start checked; people who are gone stay
 * listed while they are still unsubscribed or muted, so they can be cleared.
 */
export function alertSenders(data: AlertSettings): {senders: AlertSender[]; unsubscribed: Set<string>} {
  const unsubscribed = new Set([...data.unsubscribed_senders, ...(data.muted_senders ?? [])]);
  const relevant = new Set([...data.sharing_senders, ...unsubscribed]);
  const people = new Map(data.senders.filter(person => relevant.has(person.user)).map(person => [person.user, person]));
  for (const sender of unsubscribed) if (!people.has(sender)) people.set(sender, {user: sender, display: sender});
  const senders = [...people.values()].sort((a, b) => a.display.localeCompare(b.display) || a.user.localeCompare(b.user));
  return {senders, unsubscribed};
}
