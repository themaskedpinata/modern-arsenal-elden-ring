//! Co-op C2 (docs/COOP_PLAN.md): watches `WorldChrMan.player_chr_set` read-only at 4 Hz and logs every change of the
//! (debounced) peer set: `COOP session peers=<n> was=<m> joined=[..] left=[..] handles=[..] npc=<k> lobby=<s>
//! role=<host|client|unknown> sess_players=<n>`. The pure rules live in `mw2er_core::coop`. Never writes anything,
//! never reads a Steam id or name.

use mw2er_core::coop::{PeerTracker, READ_EVERY_S, role_label, scan};

use crate::game::chars::{player_set_slots, session_state};
use crate::log::log_i;

#[derive(Default)]
pub struct CoopCtl {
    tracker: PeerTracker,
    next_read: f64,
    first_logged: bool,
}

impl CoopCtl {
    /// The debounced number of peers.
    pub fn peers(&self) -> u32 {
        self.tracker.peers()
    }

    /// C3 PASS C: the debounced peers' slots (address, handle) for markers, poses and remote twins.
    pub fn peer_slots(&self) -> Vec<mw2er_core::coop::PeerSlot> {
        self.tracker.slots().to_vec()
    }

    /// The role from the session manager, read on demand (cheap: two integers).
    pub fn role(&self) -> &'static str {
        role_label(session_state().0)
    }

    pub fn tick(&mut self, now: f64, in_world: bool) {
        if now < self.next_read {
            return;
        }
        self.next_read = now + READ_EVERY_S;
        let sc = if in_world { player_set_slots().and_then(|(slots, main, mh, sinner)| scan(&slots, main, mh, sinner)) } else { None };
        if !self.first_logged
            && let Some(s) = sc.as_ref()
        {
            // one line per boot: the solo baseline (peers=0 expected), so a run log always shows the detector ran
            self.first_logged = true;
            let (lobby, n) = session_state();
            log_i!(
                "COOP",
                "watch slots={} peers_seen={} npc={} lobby={} role={} sess_players={}",
                s.slots,
                s.peers.len(),
                s.npc_players,
                lobby.map_or("none".to_string(), |v| v.to_string()),
                role_label(lobby),
                n.map_or("none".to_string(), |v| v.to_string())
            );
        }
        if let Some(c) = self.tracker.update(now, sc.as_ref()) {
            let (lobby, n) = session_state();
            log_i!(
                "COOP",
                "session {} lobby={} role={} sess_players={}",
                c.line(),
                lobby.map_or("none".to_string(), |v| v.to_string()),
                role_label(lobby),
                n.map_or("none".to_string(), |v| v.to_string())
            );
        }
    }
}
