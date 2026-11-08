//! Notifications with buttons on macOS (the UserNotifications framework),
//! for the offer to drop time away.
//!
//! macOS only lets an app bundle post these: run as enoch.app it works, run
//! as a bare binary `Notifier::new` gives `None` and the menu bar title
//! (`away 23m`) carries the offer alone.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_foundation::{NSArray, NSBundle, NSError, NSSet, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
    UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

use super::Action;

/// The notification category with the offer's buttons, and the id of the
/// one notification showing it (a new offer replaces the old).
const AWAY: &str = "away";

/// Button ids and labels, as on Linux.
const BUTTONS: [(&str, &str, Action); 3] = [
    ("drop", "Drop it, keep tracking", Action::DropAway),
    ("stop", "Drop it and stop", Action::DropAwayAndStop),
    ("keep", "Keep it", Action::KeepAway),
];

pub struct Notifier {
    center: Retained<UNUserNotificationCenter>,
    /// The center holds its delegate weakly.
    _delegate: Retained<Delegate>,
}

impl Notifier {
    /// Set up the offer's notification, calling `on_action` when one of its
    /// buttons is pressed. Asks for permission to notify the first time.
    pub fn new(on_action: impl Fn(Action) + 'static) -> Option<Self> {
        // UserNotifications raises an exception in a process that isn't an
        // app bundle.
        NSBundle::mainBundle().bundleIdentifier()?;
        let center = UNUserNotificationCenter::currentNotificationCenter();

        let delegate = Delegate::new(Box::new(move |id: &str| {
            if let Some((_, _, action)) = BUTTONS.iter().find(|(key, _, _)| *key == id) {
                on_action(action.clone());
            }
        }));
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        let buttons: Vec<Retained<UNNotificationAction>> = BUTTONS
            .iter()
            .map(|(key, label, _)| {
                UNNotificationAction::actionWithIdentifier_title_options(
                    &NSString::from_str(key),
                    &NSString::from_str(label),
                    UNNotificationActionOptions::empty(),
                )
            })
            .collect();
        let category =
            UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                &NSString::from_str(AWAY),
                &NSArray::from_retained_slice(&buttons),
                &NSArray::new(),
                UNNotificationCategoryOptions::empty(),
            );
        center.setNotificationCategories(&NSSet::from_retained_slice(&[category]));

        let answered = RcBlock::new(|granted: Bool, error: *mut NSError| {
            if !granted.as_bool() {
                eprintln!("enoch tray: may not notify{}", describe(error));
            }
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &answered,
        );
        Some(Self {
            center,
            _delegate: delegate,
        })
    }

    /// Show the offer, replacing any earlier one.
    pub fn show(&self, title: &str, body: &str) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        content.setCategoryIdentifier(&NSString::from_str(AWAY));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(AWAY),
            &content,
            None,
        );
        let added = RcBlock::new(|error: *mut NSError| {
            if !error.is_null() {
                eprintln!("enoch tray: notifying{}", describe(error));
            }
        });
        self.center
            .addNotificationRequest_withCompletionHandler(&request, Some(&added));
    }

    /// Take the offer's notification away, once it's settled from the menu.
    pub fn withdraw(&self) {
        let ids = NSArray::from_retained_slice(&[NSString::from_str(AWAY)]);
        self.center
            .removePendingNotificationRequestsWithIdentifiers(&ids);
        self.center
            .removeDeliveredNotificationsWithIdentifiers(&ids);
    }
}

/// `: <reason>` for a framework error, or nothing.
fn describe(error: *mut NSError) -> String {
    // SAFETY: the framework passes either null or a valid NSError.
    match unsafe { error.as_ref() } {
        Some(e) => format!(": {}", e.localizedDescription()),
        None => String::new(),
    }
}

struct Ivars {
    on_action: Box<dyn Fn(&str)>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and Delegate does
    // not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "EnochNotificationDelegate"]
    #[ivars = Ivars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// Show the offer even while enoch is the active app.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion_handler
                .call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List,));
        }

        /// A button was pressed (or the notification clicked or dismissed,
        /// which leaves the offer in the menu).
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive_response(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &block2::DynBlock<dyn Fn()>,
        ) {
            (self.ivars().on_action)(&response.actionIdentifier().to_string());
            completion_handler.call(());
        }
    }
);

impl Delegate {
    fn new(on_action: Box<dyn Fn(&str)>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { on_action });
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}
