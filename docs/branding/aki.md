# Aki, the AnkiQuest study companion

Aki is an orange-and-cream red panda with an indigo scarf, a flashcard satchel,
and a ringed tail. The original artwork was generated for AnkiQuest with
OpenAI's image-generation tool and approved by the project owner. Production
prompts are recorded beside this document. The PNG originals have transparent
backgrounds and are approximately 1280 × 1280 pixels.

## Where Aki appears

- Website and installed web-app branding: navigation, sign-in, favicon and icon.
- Profiles: encouragement reflecting today's actual quests and streak state.
- Community, records and trophies: headers, loading and empty states.
- Streak protection: a cozy freeze illustration in its settings dialog.
- Desktop add-on: a locally served deck-list image and settings dialog icon.

The seven assets in `static/aki` are embedded in the server binary. Only their
exact public paths bypass sign-in; member APIs and unknown paths remain
protected. The desktop face is packaged locally in `addon/aki_face.png` so
branding works offline.

## Pose and voice rules

| Asset | Use |
| --- | --- |
| `welcome.png` | Onboarding, sign-in and empty states |
| `review.png` | A small next step, loading and studying |
| `celebrate.png` | Confirmed completed daily quests and conquered cards |
| `streak.png` | Studying today and general progress |
| `freeze.png` | Confirmed protection; contextual settings illustration |
| `winner.png` | Records, trophies, winners and tamed leeches |
| `face.png` | Compact branding and installed app icon |

Aki encourages small steps and celebrates confirmed progress. Avoid guilt,
threats, fake rewards or implying a streak is protected merely because a user
owns freezes. An expired daily profile receives neutral study encouragement.
Contextual artwork on a settings dialog is decorative, not a protection status.

Artwork has empty alternative text and is excluded from interactive controls.
Website encouragement is translated into English, Spanish, French, German and
Portuguese; the desktop add-on currently translates English and Spanish.
Useful encouragement is ordinary translated text. Keep images static, allow
text to wrap, preserve clear space and retain player avatars and names.

Native Android assets ship with the companion Android change, so the app does
not require a network request to display Aki. No review weights, permissions,
scoring, notification timing or user preferences are changed by this branding.
