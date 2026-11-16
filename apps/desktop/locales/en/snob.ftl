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

rail = Sections
home-title = Home
search-title = Search
search-note = Look up an account, its posts and its lists. Not ready yet.
automations-title = Automations
automations-intro = A space and an editor for automations: flows that start on a trigger, go on when their conditions hold, and reach other apps through webhooks or APIs.
automations-begin = Get started
automations-search = Search automations
automations-create = Create
automations-empty = No automations yet
automations-empty-note = Create one to watch accounts and act when something changes.
automations-none = No automation is called “{ $typed }”
editor-title = New automation
editor-back = Back to automations
editor-canvas = The automation's canvas
editor-start = Choose a trigger to start
editor-hint = Ctrl + drag or the middle button to move, the wheel to zoom
editor-zoom = Zoom
editor-zoom-in = Zoom in
editor-zoom-out = Zoom out
editor-zoom-level = { $percent } %
editor-zoom-reset = Zoom at { $percent } %. Back to 100 %
editor-fit = Back to the middle
editor-steps = Steps
editor-steps-title = Add a step
editor-soon = Coming soon
editor-triggers = Triggers
editor-conditions = Conditions
editor-actions = Actions
trigger-schedule = On a schedule
trigger-schedule-note = Every day, every week or at a time you choose.
trigger-followers = When followers change
trigger-followers-note = Somebody follows an account or stops following it.
trigger-webhook = On a webhook call
trigger-webhook-note = Another app calls an address of the automation's own.
condition-filter = Filter
condition-filter-note = Go on only when what came in matches.
condition-branch = Branch
condition-branch-note = Take one of two paths.
action-webhook = Send to a webhook
action-webhook-note = Post what happened to an address of yours.
action-api = Call an API
action-api-note = Send a request to a service you use.
action-notify = Notify me
action-notify-note = A notification on this computer.
settings-title = Settings
settings-search = Search settings
settings-none = No setting matches “{ $typed }”
settings-app = App
settings-theme = Theme
settings-theme-note = How snob looks, whatever the system is set to.
settings-theme-keywords = appearance color mode dark light
theme-system = System
theme-light = Light
theme-dark = Dark
settings-language = Language
settings-language-note = The language of every word in the app.
settings-language-keywords = translation locale english spanish
language-system = System language
language-en = English
language-es = Español
settings-logs = Logs
settings-logs-note = Kept on this computer. The report makes accounts anonymous.
settings-logs-keywords = log logging debug problem issue report support
settings-storage = Storage
settings-storage-total = snob keeps { $size } on this computer
settings-storage-measuring = Measuring what is kept
settings-storage-note = Measured from the files' sizes, none opened.
settings-storage-keywords = space disk size files data
storage-databases = Databases
storage-databases-note = Each account's lists, walks and history.
storage-cache = Cache
storage-cache-note = What a view shows at once. Fetched again when needed.
storage-browsers = Browser profiles
storage-browsers-note = The browser each account's requests leave from.
storage-jobs = Jobs
storage-jobs-note = Lists walked in the background, and their results.
storage-logs = Logs
storage-logs-note = The app's log and its crash reports.
storage-update = Update
storage-update-note = A newer version, downloaded and waiting.
storage-scratch = Stories opened
storage-scratch-note = Files handed to the system's viewer.
storage-other = Other
storage-other-note = Sessions kept in files, settings and locks.
storage-account-note = Signed in here
storage-account-signed-out = Not signed in: its data is still here
storage-account-sizes = History { $history } · Cache { $cache }
storage-account-clear = Clear cache
storage-account-clear-named = Clear the cache of { $account }
storage-account-forget = Delete history
storage-account-forget-named = Delete the history of { $account }
storage-working = Under way
storage-account-keywords = delete erase remove forget history clean
storage-disk-of = On a disk of { $size }
storage-disk-others = Everything else
storage-disk-snob = snob
storage-disk-free = Free
storage-disk-said = On a disk of { $size }: { $others } everything else, { $snob } snob, { $free } free
storage-disk-unknown = The disk did not say how large it is.
settings-accounts = Accounts
settings-accounts-note = What each account keeps here. Its session, its browser and its pace are never touched.
settings-forget-all = Delete all history
forget-title = Delete the history of { $account }?
forget-title-all = Delete the history of every account?
forget-body = Its stored lists and captures, what the monitor saw, its jobs and its cache are deleted from this computer. This cannot be undone. Its session, its browser and its pace stay: it stays signed in and sends nothing sooner.
forget-body-all = Every account's stored lists and captures, what the monitor saw, their jobs and their cache are deleted from this computer. This cannot be undone. Their sessions, their browsers and their pace stay: they stay signed in and send nothing sooner.
forget-confirm = Delete history
forget-done = { $size } freed
forget-busy = { $size } freed. Accounts left as they were because a job or a run of theirs is going: { $count }.
forget-failed = { $size } freed. Accounts whose history could not be deleted: { $count }. The log says why.
settings-outline = The sections of the settings
settings-clear-cache = Clear all cache
settings-cleared = { $size } freed
settings-cleared-partly = { $size } freed. Some of the cache could not be removed: the log says where.
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
