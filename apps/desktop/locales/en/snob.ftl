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
status-starting = Preparing snob's engine…
status-loading = Loading the profile…
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
automation-watch-name = snob watch
automation-new-name = New automation
automation-steps = Steps: { $count }
automation-back = Back to the list
automation-edit = Edit the automation
automation-run = Run now
automation-started = Started. The run shows in the history once it begins.
automation-not-started = It could not be started: { $why }
automation-history = History
automation-reading = Reading the history…
automation-no-runs = It has not run yet.
automation-flow = Flow
automation-empty-flow = Nothing is on its canvas yet. Edit it to add steps.
automation-next-now = Due now.
automation-next-at = Next run: { $when }
automation-next-never = No next run: it has no schedule that can run.
automation-next-by-hand = It runs when you run it.
run-ok = Finished
run-error = Failed
run-rate-limited = Paused by Instagram
run-no-session = Not signed in
run-challenge = Needs verifying
run-interrupted = Stopped
run-unfinished = Did not finish
run-requests = Requests: { $count }
stage-trigger = Starts
stage-action = Reads
stage-filter = Keeps
stage-output = Sends to
node-every = Every { $every }
node-on = On { $days }
node-at = At { $times }
node-cron = Cron { $expression }
node-no-schedule = No schedule yet
node-no-account = Whose account is not said
node-own-account = Your account
node-manual = When you run it
node-no-address = No address yet
node-disabled = Disabled
node-has-problem = Something keeps it from running
node-menu = The step's menu
node-menu-disable = Disable
node-menu-enable = Enable
node-menu-configure-trigger = Trigger configuration
node-menu-configure = Configure
node-menu-connect = Connect to { $step }
node-menu-disconnect = Disconnect from { $step }
node-menu-remove = Remove
unjoined-missing = One of those steps is not there.
unjoined-itself = A step cannot lead to itself.
unjoined-already = Those two steps are already joined.
unjoined-wrong-way = A step of that kind cannot follow that one.
unjoined-circle = That would make the flow go round in a circle.
problem-no-name = It has no name.
problem-no-trigger = It has no trigger, so nothing would ever start it.
problem-no-action = No trigger leads to an action, so a run would read nothing.
problem-schedule = The schedule cannot be used: { $why }
problem-no-account = An action does not say whose account it reads.
problem-bad-name = “{ $name }” cannot be a username.
problem-no-consent = Reading the lists of @{ $name } needs your yes, given in the step's settings.
problem-own-account = This action is about another account: say whose.
problem-relative-dir = The folder to save in is not a whole path.
problem-no-address = The webhook has no address it can use.
problem-two-webhooks = An automation sends to one webhook at most.
problem-taken = The { $what } of @{ $name } are already read by “{ $by }”.
problem-not-in-watch-toml = watch.toml cannot hold this step; make it in an automation of its own.
editor-title = Edit automation
editor-name = Name of the automation
editor-undo = Undo
editor-redo = Redo
editor-back = Back to the automation
editor-canvas = The automation's canvas
editor-start = Add a trigger to start
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
editor-actions = Actions
editor-filters = Filters
editor-outputs = Outputs
editor-triggers-note = What starts it.
editor-actions-note = What it reads.
editor-filters-note = Which accounts go on.
editor-outputs-note = Where what it finds goes.
editor-shelf-back = Back to the kinds of step
editor-shelf-search = Search these steps
editor-shelf-search-hint = Search
editor-shelf-none = No step matches “{ $typed }”
editor-shelf-first = Every automation starts with a trigger.
editor-item-add = Add: { $name }
editor-item-soon = { $name }, coming soon
editor-not-kept = The automation could not be kept: { $why }
trigger-schedule = On a schedule
trigger-schedule-note = Every few hours, on some days, or at times you choose.
trigger-manual = Manually
trigger-manual-note = Only when you run it, with “Run now”.
trigger-webhook = On a webhook call
trigger-webhook-note = Another app calls an address of the automation's own.
read-lists = Followers and following
read-lists-note = Who came and who went since the last run.
read-unfollowers = Who does not follow back
read-unfollowers-note = The accounts it follows that do not follow it.
read-fans = Fans
read-fans-note = The accounts that follow it and that it does not follow.
read-friends = Friends
read-friends-note = The accounts it follows that follow it back.
read-posts = New posts
read-posts-note = Its new posts, and their photos and videos, saved in a folder.
read-picture = Profile picture
read-picture-note = Its picture at the best size, saved when it changes.
read-known-followers = Followed by people you follow
read-known-followers-note = The accounts you follow that follow it.
read-friends-in-common = Friends in common
read-friends-in-common-note = Its friends who are also yours.
filter-changes = Only what changed
filter-changes-note = Who came and who went, not everybody.
filter-verified = Verified accounts
filter-verified-note = Only the accounts with the verified badge.
filter-unverified = Unverified accounts
filter-unverified-note = Leave out the verified ones.
filter-public = Public accounts
filter-public-note = Leave out the private ones.
filter-private = Private accounts
filter-private-note = Only the private ones.
output-webhook = Send to a webhook
output-webhook-note = Post what happened to an address of yours.
output-api = Call an API
output-api-note = Send a request to a service you use.
output-notify = Notify me
output-notify-note = A notification on this computer.
schedule-every-hint = 6h, 2d
schedule-on-hint = mon, thu
schedule-at-hint = 09:00, 18:30
schedule-cron-hint = 0 9 * * *
schedule-jitter-hint = 5m
action-username-hint = The account's username
action-folder-hint = A folder on this computer
webhook-url-hint = https://example.com/hook
webhook-header-name-hint = Name
webhook-header-value-hint = Value
dialog-schedule-title = Trigger configuration
dialog-schedule-note = Say when it runs: an interval, days and times, or a cron expression. An interval beside days or times is the least wait between two runs.
dialog-configure-title = Configure: { $name }
dialog-save = Save
schedule-every = Every
schedule-on = On these days
schedule-at = At these times
schedule-cron = Cron expression
schedule-jitter = Random delay of up to
filter-choice = What it lets through
action-whose = Whose account
action-mine = Your account
action-other = Another account
action-username = Username
action-consent = I agree that this automation reads the followers and the following of @{ $name } each time it runs.
action-consent-note = Another account's lists are read slowly, a few at a time, and count toward the day's limit.
action-folder = Folder to save in
action-choose = Choose
action-folder-note = Files are saved there under names of their own, and none is written over. Leave it empty to save nothing.
action-quality = Video quality
action-best = Best
action-compat = Compatible (720p)
webhook-url = Address
webhook-format = What is sent
webhook-format-json = Report (JSON)
webhook-format-slack = Slack
webhook-format-discord = Discord
webhook-format-ntfy = ntfy
webhook-format-text = Plain text
webhook-heartbeat = Also tell it when nothing changed
webhook-heartbeat-note = A short message on every run, so you know it ran.
webhook-headers = Headers
webhook-headers-note = A header here is kept in a plain file on this computer. It is not a place for a password or a token.
webhook-header-name = Header name
webhook-header-value = Header value
webhook-header-remove = Remove this header
webhook-header-add = Add a header
leave-title = Leave without finishing?
leave-body = This automation cannot run yet:
leave-problems = What is wrong
leave-discard = Discard changes
leave-keep = Keep editing
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

## A profile, its lists and its picture. Nothing here opens a post or a
## story, or marks anything as seen.
account-profile = Your profile
profile-title = { $name } ({ $account })
profile-picture-open = Open the profile picture of @{ $username }
profile-picture-story = Profile picture of @{ $username }, with a story
profile-picture-menu = Story or profile picture
profile-open-story = Open story
profile-open-picture = Open profile picture
profile-story-later = Stories cannot be opened yet. Nothing was read or marked as seen.
profile-options = More options
profile-options-menu = Options
profile-copy-username = Copy username
profile-copy-link = Copy profile link
profile-copy-id = Copy account ID
profile-copied-username = Username copied
profile-copied-link = Profile link copied
profile-copied-id = Account ID copied
profile-verified = Verified
profile-private-mark = Private
profile-follows-you = Follows you
profile-followers = followers
profile-following = following
profile-followers-open = { $count } followers, open the list
profile-following-open = { $count } following, open the list
profile-highlights = Highlights
profile-highlight = Highlight: { $title }
profile-showing = Show posts as
profile-grid = Grid
profile-list = List
profile-layout-split = Put the card beside the posts
profile-layout-stacked = Put the card above the posts
profile-tabs = Posts of this profile
profile-tab-posts = Posts
profile-tab-posts-count = Posts · { $count }
profile-tab-reposts = Reposts
profile-tab-tagged = Mentioned in
profile-reading = Reading the posts
profile-private = This account is private.
profile-private-note = Its posts and lists are shown only to the accounts it accepted.
profile-no-session = No account is signed in. Sign in again to see this profile.
profile-nobody = No account goes by that name.
profile-failed = The profile could not be read. Try again later.
profile-posts-failed = The posts could not be read. Try again later.
profile-no-posts = No posts yet
profile-no-reposts = No reposts
profile-no-tagged = Not mentioned in any post
profile-photo = Photo
profile-video = Video
profile-reel = Reel
profile-carousel = Carousel of { $count }
profile-pinned = pinned
profile-likes = { $count } likes
profile-comments = { $count } comments
profile-no-caption = No caption
profile-first-page = The first { $count } posts are shown.
people-followers-title = Followers of { $account }
people-following-title = Accounts { $account } follows
people-reading = Reading… { $count } so far
people-reading-of = Reading… { $count } of { $total }
people-read = { $count } accounts
people-stored = As stored at { $time }.
people-not-whole = The list was not read to its end.
people-detach = Open in a new window
people-list = Accounts
people-open = Open the profile of @{ $username }
list-question-followers = Read the followers of { $account }?
list-question-following = Read the accounts { $account } follows?
list-question-body = snob reads this list from Instagram at a human pace, a page at a time, as you would scrolling it. A long list takes a while, and nothing is sent to the account.
list-question-confirm = Read the list
viewer-title = Profile picture of @{ $username }, large
viewer-download = Save to Downloads
viewer-copy = Copy picture
viewer-close = Close the picture
viewer-menu = Picture options
viewer-menu-copy = Copy to clipboard
viewer-menu-save = Save as…
viewer-saved = Saved as { $file }
viewer-not-saved = The picture could not be saved.
viewer-copied = Picture copied
viewer-not-copied = The picture could not be copied.

## Shared: what several screens say the same way. A count of one has a
## message of its own, its id and `-one` (`Look::tr_count`): every value is
## put in as a string, which never selects a Fluent variant.

count-accounts = { $count } accounts
count-accounts-one = { $count } account
list-and = { $first } and { $last }
month-jan = January
month-feb = February
month-mar = March
month-apr = April
month-may = May
month-jun = June
month-jul = July
month-aug = August
month-sep = September
month-oct = October
month-nov = November
month-dec = December
month-short-jan = Jan
month-short-feb = Feb
month-short-mar = Mar
month-short-apr = Apr
month-short-may = May
month-short-jun = Jun
month-short-jul = Jul
month-short-aug = Aug
month-short-sep = Sep
month-short-oct = Oct
month-short-nov = Nov
month-short-dec = Dec
weekday-mon = Monday
weekday-tue = Tuesday
weekday-wed = Wednesday
weekday-thu = Thursday
weekday-fri = Friday
weekday-sat = Saturday
weekday-sun = Sunday
weekday-short-mon = Mon
weekday-short-tue = Tue
weekday-short-wed = Wed
weekday-short-thu = Thu
weekday-short-fri = Fri
weekday-short-sat = Sat
weekday-short-sun = Sun
nth-first = first
nth-second = second
nth-third = third
nth-fourth = fourth
nth-last = last
# How this language tells the time of day: 12 or 24 hours.
clock-hours = 12
clock-24 = { $hour }:{ $minute }
clock-am = { $hour }:{ $minute } AM
clock-pm = { $hour }:{ $minute } PM
day-today = today
day-yesterday = yesterday
date-this-year = { $month } { $day }
date-other-year = { $month } { $day }, { $year }
# A moment is said with its hour: `-one` is for the hour 1, which some
# languages say in the singular.
moment-today = Today at { $time }
moment-today-one = Today at { $time }
moment-yesterday = Yesterday at { $time }
moment-yesterday-one = Yesterday at { $time }
moment-this-week = { $weekday } at { $time }
moment-this-week-one = { $weekday } at { $time }
moment-this-year = { $date } at { $time }
moment-this-year-one = { $date } at { $time }
moment-other-year = { $date } at { $time }
moment-other-year-one = { $date } at { $time }
stepper-less = Fewer: { $what }
stepper-more = More: { $what }

## Profile: a profile's page, its options and what it tells of the account.

profile-showing-grid = Grid view
profile-showing-list = List view
profile-username-copy = Copy @{ $username }
profile-friend = Friend
profile-open-browser = Open in the browser
profile-opened-browser = Opened in your browser
profile-about = About this account
profile-about-joined = Date joined
profile-about-joined-in = { $month } { $year }
profile-about-based = Account based in
profile-about-usernames = Username changes
profile-about-unread = Not read yet
profile-about-note = Instagram shows these in its own app. snob does not read them yet, and makes none of them up.
profile-about-close = Close
profile-reading-more = Reading more posts…
profile-resting = snob rests until { $time } before reading more, at a person's pace.
profile-more-failed = More posts could not be read.
profile-more-again = Try again
profile-most-pages = snob reads at most { $count } pages of a tab in one visit.
profile-all-read = That is everything.

## People: the lists of followers and following, and choosing in them.

## Automations: the list, an automation's page and its runs, and the editor.

## Schedule: when an automation runs, chosen in its dialog and said in words.
