package desktop

import "testing"

func TestDetectsBackgroundArgument(t *testing.T) {
	if !IsBackgroundLaunch([]string{"cosmic-mail", BackgroundArg}) || !IsBackgroundLaunch([]string{"cosmic-mail", "--unrelated", BackgroundArg}) {
		t.Fatal("background")
	}
	if IsBackgroundLaunch([]string{"cosmic-mail"}) || IsBackgroundLaunch([]string{"cosmic-mail", "--background-task"}) {
		t.Fatal("normal")
	}
}
