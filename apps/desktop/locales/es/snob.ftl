# Las palabras de la app de escritorio, en español. Los mismos mensajes y
# las mismas variables que `locales/en/snob.ftl`; una prueba los compara.

brand = snob.
window-title = snob
test-badge = PRUEBA

## Los botones de la ventana, donde los dibuja la app

window-minimize = Minimizar
window-maximize = Maximizar
window-restore = Restaurar
window-close = Cerrar

## La línea de estado de la banda superior

status-offline = Sin conexión a Internet
status-paused = Instagram ha pedido una pausa · se reanuda a las { $time }
status-resuming = Reanudando…
status-downloading = Descargando actualización · { $percent } %
status-downloading-unknown = Descargando actualización
status-update-available = Hay una versión nueva

## La foto de la cuenta y su menú

account-menu = Menú de cuenta
account-photo = Foto de perfil de @{ $username }
account-photo-unnamed = Foto de perfil
account-add = Añadir cuenta
account-switch = Cambiar cuenta
account-add-another = Añadir otra cuenta
account-more = +{ $count }
account-settings = Configuración
account-sign-out = Cerrar sesión
account-switched = Ahora usas { $account }
sign-out-title = ¿Cerrar la sesión de { $account }?
sign-out-body = La sesión y el perfil de navegador de snob para esta cuenta se borran de este ordenador. La sesión sigue activa en Instagram hasta que la cierres allí.
sign-out-confirm = Cerrar sesión
cancel = Cancelar
back = Volver
loading = Cargando

## La primera pantalla

start-title = Empecemos.
start-browser = Iniciar sesión en Instagram
start-paste = Usar tu ID de sesión (avanzado)
start-expired = Tu sesión ha caducado. Vuelve a iniciar sesión.

## La actualización entre elegir un método y usarlo

update-checking = Buscando actualizaciones
update-downloading = Descargando actualización
update-percent = { $percent } %
update-restarting = Reiniciando…
update-failed = No se ha podido instalar la actualización. snob sigue con esta versión.

## Iniciar sesión con la ventana de navegador de snob

browser-title = Iniciar sesión en Instagram
browser-body = Inicia sesión en Instagram en la ventana que se ha abierto.
browser-note = Es la ventana de navegador de snob, no tu navegador habitual, así que ahí todavía no has iniciado sesión.

## Iniciar sesión con un ID de sesión

paste-title = Usar tu ID de sesión
paste-field = ID de sesión
paste-show = Mostrar el ID de sesión
paste-hide = Ocultar el ID de sesión
paste-paste = Pegar
paste-submit = Iniciar sesión con este ID
paste-checking = Comprobando con Instagram
paste-hint-invalid = Copia solo el valor de la cookie sessionid.
paste-tutorial-title = Dónde encontrarlo
paste-step-1 = Abre instagram.com en un navegador donde tengas la sesión iniciada.
paste-step-2 = Abre las herramientas de desarrollo: F12, o Cmd+Opt+I en un Mac.
paste-step-3 = Ve a Aplicación › Cookies › instagram.com.
paste-step-4 = Copia el valor de sessionid.

## Cuando falla el inicio de sesión

error-title = No ha funcionado
error-offline = No hay conexión a Internet. Comprueba tu red y vuelve a intentarlo.
error-rejected = Ese ID de sesión ya no funciona. Saca uno nuevo o inicia sesión con el navegador.
error-rejected-browser = Instagram no ha aceptado esa sesión. Vuelve a iniciar sesión.
error-challenge = Instagram quiere confirmar que eres tú. Inicia sesión con el navegador para completarlo.
error-paused = Instagram ha pedido una pausa. Inténtalo de nuevo más tarde.
error-paused-until = Instagram ha pedido una pausa. Inténtalo de nuevo después de las { $time }.
error-no-browser = snob necesita Chrome, Edge, Brave o Chromium instalado para hablar con Instagram.
error-timed-out = Nadie ha iniciado sesión en diez minutos. Vuelve a intentarlo cuando quieras.
error-malformed = Eso no es un ID de sesión. Copia solo el valor de la cookie sessionid.
error-unexpected = Algo ha fallado. Vuelve a intentarlo.
error-retry = Reintentar
error-details = Ver detalles
error-code = Código { $id }

## La bienvenida

welcome = Te damos la bienvenida, { $account }.

## Inicio y configuración

home-title = Inicio
settings-title = Configuración
settings-logs = Registros
settings-detailed = Registro detallado
settings-detailed-note = Para encontrar un problema. Se desactiva solo pasadas 24 horas.
settings-detailed-until = Activado hasta las { $time }
settings-open-logs = Abrir carpeta de registros
settings-copy-report = Copiar informe de diagnóstico
settings-report-copied = Informe de diagnóstico copiado
crash-title = snob se cerró de forma inesperada la última vez
crash-body = Se ha guardado un informe en este ordenador. No se ha enviado nada a ningún sitio.
crash-view = Ver el informe
crash-copy = Copiar el informe
crash-dismiss = Descartar
crash-copied = Informe copiado

## La paleta de comandos

palette-title = Paleta de comandos
palette-placeholder = Escribe un comando
palette-empty = Ningún comando coincide
