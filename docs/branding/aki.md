# Aki, the AnkiQuest study companion

Aki is an orange-and-cream red panda with an indigo scarf, a flashcard satchel,
and a ringed tail. the original artwork was generated for AnkiQuest with
OpenAI's image-generation tool and approved by the project owner. production
prompts are recorded beside this document. the PNG originals have transparent
backgrounds and are approximately 1280 × 1280 pixels.

## where Aki appears

- website and installed web-app branding: navigation, sign-in, favicon and icon.
- profiles: encouragement reflecting today's actual quests and streak state.
- community, records and trophies: headers, loading and empty states.
- streak protection: a cozy freeze illustration in its settings dialog.
- desktop add-on: a locally served deck-list image and settings dialog icon.

the seven assets in the table below are embedded in the server binary. only their
exact public paths bypass sign-in; member APIs and unknown paths remain
protected. the desktop face is packaged locally in `addon/aki_face.png` so
branding works offline.

the Android app ships WebP copies of the Aki and Ankilope art in `drawable-nodpi`. after changing a PNG in `static/`, run `python tools/export_android_art.py <android-checkout>` (Pillow) to rewrite them, and `--check` to confirm every drawable exists at its size.

The [mascot source plan](mascot-source.md) moves `aki-prompts.json` with the
PNG masters to `art/mascots/aki/` and verifies Android's generated WebPs against
that source.

## pose and voice rules

| Asset | Use |
| --- | --- |
| `welcome.png` | Onboarding, sign-in and empty states |
| `review.png` | A small next step, loading and studying |
| `celebrate.png` | Confirmed completed daily quests and conquered cards |
| `streak.png` | Studying today and general progress |
| `freeze.png` | Confirmed protection; contextual settings illustration |
| `winner.png` | Records, trophies, winners and tamed leeches |
| `face.png` | Compact branding and installed app icon |
| `face-192.png`, `face-32.png` | `face.png` scaled down for the website's brand mark, touch icon and favicon (also `ankilope/face-192.png`); scale them again if the face changes |

### moment art (#88)

these PNGs (1254 × 1254, transparent) cover moments that the seven poses above
only approximated. they are source art for the Android app's `drawable-nodpi`
names in brackets, written by the export tool. the website does not serve them
yet: they are not embedded in the server binary and are not public paths.

| Asset | Android name | Use |
| --- | --- | --- |
| `icon_happy.png` | `aki_icon_happy` | App icon mood: today counts |
| `icon_worried.png` | `aki_icon_worried` | App icon mood: streak ends soon, today doesn't count yet |
| `icon_frozen.png` | `aki_icon_frozen` | App icon mood: a freeze protects the streak |
| `icon_sleepy.png` | `aki_icon_sleepy` | App icon mood: no streak running |
| `streak_hot.png` | `aki_streak_hot` | Streak of 7+ days |
| `streak_blazing.png` | `aki_streak_blazing` | Streak of 30+ days |
| `streak_legend.png` | `aki_streak_legend` | Streak of 100+ days |
| `streak_milestone.png` | `aki_streak_milestone` | A streak milestone reached |
| `freeze_used.png` | `aki_freeze_used` | A freeze just saved the streak |
| `comeback.png` | `aki_comeback` | Starting again after a lost streak |
| `minimum.png` | `aki_minimum` | Daily minimum reached; today counts |
| `goal.png` | `aki_goal` | Daily goal reached |
| `achievement.png` | `aki_achievement` | An achievement unlocked |
| `deck_conquered.png` | `aki_deck_conquered` | A deck conquered |
| `sleeping.png` | `aki_sleeping` | Rest day, notifications off |
| `../duo/friends.png` | `duo_friends` | Aki and Ankilope together: sharing, challenges, nudges |

the prompts for the moment art were not recorded, so `aki-prompts.json` has no
entries for them yet.

Aki encourages small steps and celebrates confirmed progress. avoid guilt,
threats, fake rewards or implying a streak is protected merely because a user
owns freezes. an expired daily profile receives neutral study encouragement.
contextual artwork on a settings dialog is decorative, not a protection status.

artwork has empty alternative text and is excluded from interactive controls.
website encouragement is translated into English, Spanish, French, German and
Portuguese; the desktop add-on currently translates English and Spanish.
useful encouragement is ordinary translated text. keep images static, allow
text to wrap, preserve clear space and retain player avatars and names.

Native Android assets ship with the companion Android change, so the app does
not require a network request to display Aki. no review weights, permissions,
scoring, notification timing or user preferences are changed by this branding.
