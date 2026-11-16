# The words of the desktop app, in English: the language of the code, and
# what every other language falls back to. `locales/es/snob.ftl` holds the
# same messages with the same variables; a test compares the two.

brand = snob.
window-title = snob
test-badge = TEST

## The window's own buttons, where the app draws them

window-minimize = Minimize
window-maximize = Maximize
window-restore = Restore
window-close = Close

## The line of status in the top band

status-offline = No internet connection
status-paused = Instagram asked for a pause · resumes at { $time }
status-resuming = Resuming…
status-downloading = Downloading update · { $percent } %
status-downloading-unknown = Downloading update
status-update-available = A new version is available

## The account's picture and its menu

account-menu = Account menu
account-photo = Profile picture of @{ $username }
account-photo-unnamed = Profile picture
account-add = Add account
account-switch = Switch account
account-add-another = Add another account
account-more = +{ $count }
account-settings = Settings
account-sign-out = Sign out
account-switched = Now using { $account }
sign-out-title = Sign out of { $account }?
sign-out-body = The session and snob's browser profile for this account are removed from this computer. The session stays active on Instagram until you close it there.
sign-out-confirm = Sign out
cancel = Cancel
back = Back
loading = Loading

## The first screen

start-title = Let's get started.
start-browser = Sign in to Instagram
start-paste = Use your session ID (advanced)
start-expired = Your session has expired. Sign in again.

## The update between choosing a method and using it

update-checking = Checking for updates
update-downloading = Downloading update
update-percent = { $percent } %
update-restarting = Restarting…
update-failed = The update could not be installed. snob continues with this version.

## Signing in with snob's browser window

browser-title = Sign in to Instagram
browser-body = Sign in to Instagram in the window that has opened.
browser-note = It is snob's own browser window, not your usual browser, so you are not signed in there yet.

## Signing in with a session ID

paste-title = Use your session ID
paste-field = Session ID
paste-show = Show the session ID
paste-hide = Hide the session ID
paste-paste = Paste
paste-submit = Sign in with this ID
paste-checking = Checking with Instagram
paste-hint-invalid = Copy only the value of the sessionid cookie.
paste-browser-one = { $browser } detected
paste-browser-choose = Browser this session comes from
paste-browser-none = No browser snob can use was found
paste-tutorial-title = Where to find it
paste-step-1 = Open instagram.com in a browser where you are signed in.
paste-step-2 = Open the developer tools: F12, or Cmd+Opt+I on a Mac.
paste-step-3 = Go to Application › Cookies › instagram.com.
paste-step-4 = Copy the value of sessionid.

## When signing in fails

error-title = That did not work
error-offline = There is no internet connection. Check your network and try again.
error-rejected = That session ID no longer works. Get a new one, or sign in with the browser.
error-rejected-browser = Instagram did not accept that session. Sign in again.
error-challenge = Instagram wants to confirm it is you. Sign in with the browser to complete it.
error-paused = Instagram asked for a pause. Try again later.
error-paused-until = Instagram asked for a pause. Try again after { $time }.
error-no-browser = snob needs Chrome, Edge, Brave or Chromium installed to talk to Instagram.
error-timed-out = Nobody signed in within ten minutes. Try again when you are ready.
error-malformed = That is not a session ID. Copy only the value of the sessionid cookie.
error-unexpected = Something went wrong. Try again.
error-retry = Try again
error-details = Details
error-code = Code { $id }

## The welcome

welcome = Welcome, { $account }.

## Home and settings

home-title = Home
search-title = Search
search-note = Look up an account, its posts and its lists. Not ready yet.
automations-title = Automations
automations-note = Watch accounts and be told when something changes. Not ready yet.
settings-title = Settings
settings-logs = Logs
settings-detailed = Detailed logging
settings-detailed-note = For finding a problem. It turns itself off after 24 hours.
settings-detailed-until = On until { $time }
settings-open-logs = Open the logs folder
settings-copy-report = Copy the diagnostic report
settings-report-copied = Diagnostic report copied
crash-title = snob closed unexpectedly last time
crash-body = A report was kept on this computer. Nothing was sent anywhere.
crash-view = View the report
crash-copy = Copy the report
crash-dismiss = Dismiss
crash-copied = Report copied

## The command palette

palette-title = Command palette
palette-placeholder = Type a command
palette-empty = No command matches
palette-switch = Switch to { $account }

## Notifications: only for what happens while the window is not looked at.
notify-expired-title = Your session has expired
notify-expired-body = Sign in again as { $account } to keep using snob.
notify-expired-body-unnamed = Sign in again to keep using snob.
notify-update-title = A new version of snob is available
notify-update-body = Update it the way you installed it.
