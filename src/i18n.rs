//! Russian, when the locale or `interface.language` asks for it; English otherwise.

use std::sync::atomic::{AtomicBool, Ordering};

/// Set at start and again when the config changes.
static RUSSIAN: AtomicBool = AtomicBool::new(false);

pub fn init(configured: &str) {
    let russian = match configured {
        "ru" => true,
        "en" => false,
        _ => ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
            .is_some_and(|value| value.starts_with("ru")),
    };
    RUSSIAN.store(russian, Ordering::Relaxed);
}

pub fn russian() -> bool {
    RUSSIAN.load(Ordering::Relaxed)
}

pub fn tr(text: &'static str) -> &'static str {
    if !russian() {
        return text;
    }
    match text {
        "Calendar" => "Календарь",
        "Control center" => "Центр управления",
        "Launcher" => "Приложения",
        "Clipboard" => "Буфер обмена",
        "Notifications" => "Уведомления",
        "Weather" => "Погода",
        "Session" => "Сеанс",
        "Lock" => "Заблокировать",
        "Log out" => "Выйти",
        "Suspend" => "Сон",
        "Restart" => "Перезагрузить",
        "Power off" => "Выключить",
        "Volume" => "Громкость",
        "Microphone" => "Микрофон",
        "Microphone off" => "Микрофон выключен",
        "Microphone on" => "Микрофон включён",
        "Brightness" => "Яркость",
        "Wi-Fi" => "Wi-Fi",
        "Bluetooth" => "Bluetooth",
        "Wired" => "Проводная сеть",
        "Off" => "Выкл.",
        "on" => "вкл.",
        "off" => "выкл.",
        "On" => "Вкл.",
        "Offline" => "Нет сети",
        "Connected" => "Подключено",
        "Connecting…" => "Подключение…",
        "Password" => "Пароль",
        "Connect" => "Подключиться",
        "Forget" => "Забыть",
        "Hidden network" => "Скрытая сеть",
        "Network name" => "Имя сети",
        "Do not disturb" => "Не беспокоить",
        "No notifications" => "Уведомлений нет",
        "All quiet, nothing new" => "Всё тихо, ничего нового",
        "Settings" => "Настройки",
        "Appearance" => "Внешний вид",
        "System" => "Система",
        "Modules" => "Модули",
        "About" => "О шелле",
        "add" => "добавить",
        "a toast stays while pointed at" => "навёл — карточка не уходит",
        "Between modules" => "Между модулями",
        "bottom left" => "слева снизу",
        "bottom right" => "справа снизу",
        "by the bar" => "у панели",
        "center" => "по центру",
        "Center" => "Центр",
        "clock, calendar and graphs on the wallpaper" => "часы, календарь и графики на обоях",
        "Commands" => "Команды",
        "Config" => "Настройки",
        "Config file" => "Файл настроек",
        "Corner" => "Угол",
        "Critical stay" => "Срочные — до закрытия",
        "Date" => "Дата",
        "Desktop widgets" => "Виджеты рабочего стола",
        "Drag to reorder or to move between sections" => "Перетаскивайте, чтобы менять порядок или секцию",
        "End" => "Конец",
        "End edge" => "Край в конце",
        "Enter applies · %a is the weekday" => "Enter применяет · %a — день недели",
        "Enter applies · weather settings restart the shell" => "Enter применяет · погода перезапускает шелл",
        "Floating" => "Плавает",
        "Graphs" => "Графики",
        "h" => "ч",
        "How long" => "Сколько висят",
        "Kept in the list" => "В истории",
        "Layout" => "Раскладка",
        "Margin" => "Отступ",
        "Memory" => "Память шелла",
        "min" => "мин",
        "Mixer" => "Микшер",
        "Notification server" => "Сервер уведомлений",
        "Notifications in the list" => "Уведомлений в истории",
        "off for mako or dunst · restarts" => "выключи для mako или dunst · перезапуск",
        "off the edge, framed, with a shadow" => "отступ от края, рамка, тень",
        "off when another program hosts it · restarts" => "выключи, если трей держит другая программа · перезапуск",
        "On screen at once" => "На экране сразу",
        "or:" => "или:",
        "Pause under the pointer" => "Пауза под курсором",
        "Place" => "Место",
        "Placement" => "Расположение",
        "Quick" => "Быстро",
        "Quiet while sharing" => "Тихо при демонстрации",
        "Reread" => "Перечитать",
        "reset" => "сбросить",
        "Running" => "Работает",
        "s" => "с",
        "Screen edge" => "Край экрана",
        "Services" => "Службы",
        "Shape" => "Форма",
        "Show" => "Показать",
        "Spacing" => "Отступы",
        "Start" => "Начало",
        "Start edge" => "Край в начале",
        "straight to the list while the screen is shared" => "пока показываешь экран — сразу в историю",
        "Tiles" => "Плитки",
        "Time from the app" => "Время от программы",
        "Time on screen" => "Время на экране",
        "Toasts" => "Карточки",
        "top left" => "слева сверху",
        "top right" => "справа сверху",
        "Tray" => "Трей",
        "until closed, not timed out" => "висят, пока не закроешь",
        "What is playing" => "Что играет",
        "when an app asks for its own, up to a minute" => "если программа просит своё, до минуты",
        "When to show" => "Когда показывать",
        "Width, %" => "Ширина, %",
        "Screen sharing" => "Демонстрация экрана",
        "Ticked apps are a black box in a shared screen and in screenshots" => {
            "Отмеченные приложения в демонстрации и на скриншотах — чёрный прямоугольник"
        }
        "One window by hand:" => "Одно окно вручную:",
        "Hide window from sharing" => "Скрыть окно из демонстрации",
        "hidden" => "скрыто",
        "Accent from the wallpaper" => "Акцент из обоев",
        "a pattern between shades, not bands" => "узор между оттенками, а не полосы",
        "app and tray icons, from the background to the accent" => "значки программ и трея, от фона к акценту",
        "as it is" => "как есть",
        "Cell" => "Клетка",
        "Colour" => "Окраска",
        "Darker" => "Затемнение",
        "Dithering" => "Дизеринг",
        "Draw the wallpaper" => "Рисовать обои",
        "greys" => "серые",
        "Icons" => "Значки",
        "Icons in the theme's tones" => "Значки в тонах темы",
        "no picture" => "нет картинки",
        "over one another program draws" => "поверх обоев другой программы",
        "path to a picture" => "путь к картинке",
        "Pixels" => "Пиксели",
        "Preview" => "Как выглядит",
        "Shades" => "Оттенков",
        "Tessera's, or a path" => "из Tessera или путь",
        "through the accent" => "через акцент",
        "Wallpaper" => "Обои",
        "when the theme picks none" => "если в теме свой не выбран",
        "type" => "введите",
        "Pairing…" => "Сопряжение…",
        "Battery low" => "Батарея садится",
        "Charge it soon" => "Скоро понадобится зарядка",
        "Laptop" => "Ноутбук",
        "up" => "работает",
        "back" => "назад",
        "pair" => "сопрячь",
        "= math · 10 km in mi · > command · / files" => "= считать · 10 км в мили · > команда · / файлы",
        "# windows · @ clipboard · : emoji · ? settings" => "# окна · @ буфер · : эмодзи · ? настройки",
        "Fetching rates…" => "Загружаю курсы…",
        "Unknown currency" => "Неизвестная валюта",
        "workspace" => "стол",
        "Interface" => "Интерфейс",
        "Language" => "Язык",
        "Auto" => "авто",
        "Bar" => "Панель",
        "Position" => "Положение",
        "Top" => "сверху",
        "Bottom" => "снизу",
        "Height" => "Высота",
        "Clock" => "Часы",
        "Theme" => "Тема",
        "Accent" => "Акцент",
        "Pixel" => "Пиксель",
        "City" => "Город",
        "Moscow" => "Москва",
        "Units" => "Единицы",
        "Desktop" => "Рабочий стол",
        "Widgets" => "Виджеты",
        "Bar modules" => "Модули панели",
        "Save and restart" => "Сохранить и перезапустить",
        "Cancel" => "Отмена",
        "Set by Home Manager: programs.smalta.settings" => "Задано в Home Manager: programs.smalta.settings",
        "Clear all" => "Очистить",
        "Nothing playing" => "Ничего не играет",
        "Search…" => "Поиск…",
        "No results" => "Ничего не найдено",
        "No windows" => "Окон нет",
        "Music" => "Музыка",
        "Pixelate" => "Пикселизация",
        "big pixels and a few shades; off, the picture as it is" => {
            "крупные пиксели и мало оттенков; выкл. — картинка как есть"
        }
        "Visualizer" => "Визуализатор",
        "bars by the song on the bar, through cava" => "столбики под песней на панели, через cava",
        "Drag to select · click for the whole screen · Esc cancels" => {
            "Выделите область · клик — весь экран · Esc — отмена"
        }
        "Saved to" => "Сохранено в",
        "Screenshot copied" => "Скриншот скопирован",
        "Windows" => "Окна",
        "Empty" => "Пусто",
        "Image" => "Картинка",
        "Text" => "Текст",
        "Clear all?" => "Очистить всё?",
        "Nothing pinned" => "Ничего не закреплено",
        "Power" => "Питание",
        "Screenshot" => "Скриншот",
        "Pick colour" => "Пипетка",
        "Record" => "Запись",
        "Stop recording" => "Остановить запись",
        "Today" => "Сегодня",
        "Output" => "Вывод",
        "Input" => "Ввод",
        "Devices" => "Устройства",
        "Scanning…" => "Поиск…",
        "Back" => "Назад",
        "just now" => "только что",
        "Battery" => "Батарея",
        "Charging" => "Заряжается",
        "Feels like" => "Ощущается как",
        "Humidity" => "Влажность",
        "Wind" => "Ветер",
        "Not configured" => "Не настроено",
        "Copied" => "Скопировано",
        "Saver" => "эконом",
        "Balanced" => "баланс",
        "Performance" => "мощность",
        _ => text,
    }
}

pub fn weekday_short(index: u32) -> &'static str {
    const EN: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
    const RU: [&str; 7] = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];
    (if russian() { RU } else { EN })[index as usize % 7]
}

pub fn month(index: u32) -> &'static str {
    const EN: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    const RU: [&str; 12] = [
        "Январь",
        "Февраль",
        "Март",
        "Апрель",
        "Май",
        "Июнь",
        "Июль",
        "Август",
        "Сентябрь",
        "Октябрь",
        "Ноябрь",
        "Декабрь",
    ];
    (if russian() { RU } else { EN })[index as usize % 12]
}

pub fn weekday_long(index: u32) -> &'static str {
    const EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    const RU: [&str; 7] = ["Понедельник", "Вторник", "Среда", "Четверг", "Пятница", "Суббота", "Воскресенье"];
    (if russian() { RU } else { EN })[index as usize % 7]
}

/// The month as it reads after a day number: "9 ноября", not "9 Ноябрь".
pub fn month_of(index: u32) -> &'static str {
    const EN: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    const RU: [&str; 12] = [
        "января",
        "февраля",
        "марта",
        "апреля",
        "мая",
        "июня",
        "июля",
        "августа",
        "сентября",
        "октября",
        "ноября",
        "декабря",
    ];
    (if russian() { RU } else { EN })[index as usize % 12]
}

/// How long ago, from seconds: "9 минут назад", "9 min ago".
pub fn ago(seconds: i64) -> String {
    if seconds < 60 {
        return tr("just now").to_owned();
    }
    let (n, unit) = match seconds {
        60..3600 => (seconds / 60, 0),
        3600..86_400 => (seconds / 3600, 1),
        _ => (seconds / 86_400, 2),
    };
    if !russian() {
        return format!("{n} {} ago", ["min", "h", "d"][unit]);
    }
    const FORMS: [[&str; 3]; 3] = [["минуту", "минуты", "минут"], ["час", "часа", "часов"], ["день", "дня", "дней"]];
    let form = match (n % 10, n % 100) {
        (_, 11..=14) => 2,
        (1, _) => 0,
        (2..=4, _) => 1,
        _ => 2,
    };
    format!("{n} {} назад", FORMS[unit][form])
}
