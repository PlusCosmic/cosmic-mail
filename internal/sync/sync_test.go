package sync

import (
	"encoding/json"
	"testing"
	"time"

	"cosmicmail/internal/models"
)

func TestSyncStatePayloadSerialisesCamelCase(t *testing.T) {
	b, err := json.Marshal(models.SyncStateEvent{AccountID: "acct-1", State: models.SyncSyncing})
	if err != nil || string(b) != `{"accountId":"acct-1","state":"syncing","error":null,"needsReauth":false}` {
		t.Fatalf("%s %v", b, err)
	}
	b, _ = json.Marshal(models.SyncStateEvent{AccountID: "acct-1", State: models.SyncError, Error: models.Str("connection refused")})
	if string(b) != `{"accountId":"acct-1","state":"error","error":"connection refused","needsReauth":false}` {
		t.Fatal(string(b))
	}
}

func TestDrainAction(t *testing.T) {
	if DrainAction(false, 0) != ActionIdle || DrainAction(false, MaxConsecutiveResyncsBeforeIdle+10) != ActionIdle {
		t.Fatal("clean drain idles")
	}
	if DrainAction(true, 0) != ActionResync || DrainAction(true, MaxConsecutiveResyncsBeforeIdle-1) != ActionResync {
		t.Fatal("under the bound resyncs")
	}
	if DrainAction(true, MaxConsecutiveResyncsBeforeIdle) != ActionEndCycle || DrainAction(true, MaxConsecutiveResyncsBeforeIdle+10) != ActionEndCycle {
		t.Fatal("at the cap ends the cycle")
	}
}

func TestIdleWaitBudget(t *testing.T) {
	reissue := 5 * time.Minute
	if IdleWaitBudget(20*time.Minute, reissue) != reissue {
		t.Fatal("prefers reissue interval")
	}
	if IdleWaitBudget(90*time.Second, reissue) != 90*time.Second {
		t.Fatal("clamps to deadline")
	}
	if IdleWaitBudget(reissue, reissue) != reissue {
		t.Fatal("boundary")
	}
}

func TestIdleTimeoutAction(t *testing.T) {
	if IdleTimeoutAction(20*time.Minute, 5*time.Minute) != TimeoutReissue {
		t.Fatal("not clamped → reissue")
	}
	if IdleTimeoutAction(90*time.Second, 90*time.Second) != TimeoutEndCycle {
		t.Fatal("clamped → end cycle")
	}
	interval := 5 * time.Minute
	if IdleTimeoutAction(interval, IdleWaitBudget(interval, interval)) != TimeoutEndCycle {
		t.Fatal("boundary where the reissue interval would overrun the deadline")
	}
}
