"""Refresh all 131 PPI identities from fixed static evidence and active sources.

No game/VM/plugin is executed. This report describes implemented source routes,
not compiled or verified gameplay. Run from any current directory.
"""
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ARCHIVE = ROOT / 'archive/2026-11'
SOURCE = ROOT / 'simulator/crates/desktop/src'


def main():
    inventory = json.loads((ARCHIVE/'20261117_D_ptclpi_inventory.json').read_text(encoding='utf8'))
    classification = {r['sha256']:r for r in json.loads((ARCHIVE/'20261117_D_ppi_identity_classification.json').read_text(encoding='utf8'))}
    cfg = {r['sha256']:r for r in json.loads((ARCHIVE/'20261117_D_ppi_cfg_clusters.json').read_text(encoding='utf8'))['rows']}
    usage_path = ARCHIVE/'20261117_D_ppi_usage.json'
    usage = {r['sha256']:r for r in json.loads(usage_path.read_text(encoding='utf8'))['rows']} if usage_path.exists() else {}
    models = {}
    layouts = {
        '85cd9a': ('sdk2.rain', 'particle_plugin.rs', 'direct_layout'),
        '623e65': ('sdk2.rain', 'particle_plugin.rs', 'direct_layout'),
        '6ab38b': ('sdk2.atlas_or_scaled_rain', 'particle_rain_field.rs', 'direct_layout'),
        '350e23': ('sdk2.atlas_or_scaled_rain', 'particle_rain_field.rs', 'direct_layout'),
        '9bbce1': ('sdk2.bang2', 'particle_burst.rs', 'direct_layout'),
        'afe013': ('sdk2.common_plist', 'particle_common.rs', 'common_layout'),
        '137c56': ('sdk2.common_plist', 'particle_common.rs', 'common_layout'),
        '154233': ('sdk2.family.Path.SAKURA', 'particle_family.rs', 'is_sakura'),
        'f9c646': ('sdk2.family.Question', 'particle_family.rs', 'question_oscillation'),
        'de9de8': ('sdk2.family.Question', 'particle_family.rs', 'question_oscillation'),
        '3a412a': ('sdk2.family.Question', 'particle_family.rs', 'question_oscillation'),
        'fa9e19': ('sdk2.family.Field.SingleCenter', 'particle_family.rs', 'dark_field'),
        'bddb1b': ('sdk2.family.Field.ThreeCenters', 'particle_family.rs', 'dark_field'),
        '75b5eb': ('sdk2.family.Field.Rising', 'particle_family.rs', 'dark_field'),
        '25be67': ('sdk2.family.Field.Ground', 'particle_family.rs', 'dark_field'),
        '1694c7': ('sdk2.family.Path.NOTE', 'particle_family.rs', 'path_effect'),
        '58bd95': ('sdk2.family.Path.NOTE', 'particle_family.rs', 'path_effect'),
        '1972b7': ('sdk2.family.Path.OKO', 'particle_family.rs', 'path_effect'),
        'f4d7ac': ('sdk2.family.Path.SWEAT', 'particle_family.rs', 'path_effect'),
        'cb791b': ('sdk2.family.Path.SWEAT', 'particle_family.rs', 'path_effect'),
        'afc0eb': ('sdk2.family.Path.SIGH', 'particle_family.rs', 'path_effect'),
        '5ce268': ('sdk2.family.Path.ENERGY', 'particle_family.rs', 'path_effect'),
        '7b868e': ('sdk2.family.Path.KIRAKIRA', 'particle_family.rs', 'path_effect'),
        'aaddc4': ('sdk2.family.Path.WARUKIRA', 'particle_family.rs', 'path_effect'),
        '39bf02': ('sdk2.family.Path.SHY', 'particle_family.rs', 'path_effect'),
        '940362': ('sdk2.family.Path.STEAM', 'particle_family.rs', 'path_effect'),
        'dea3f0': ('sdk2.family.Path.STEAM', 'particle_family.rs', 'path_effect'),
        '526565': ('sdk2.family.Path.WC_STEAM', 'particle_family.rs', 'path_effect'),
        '18a982': ('sdk2.family.Path.HADOU', 'particle_family.rs', 'path_effect'),
        'd27cab': ('sdk2.family.Path.THUNDER', 'particle_family.rs', 'path_effect'),
        'c7ba77': ('sdk2.family.Path.FIREBASE', 'particle_family.rs', 'path_effect'),
        '5b59b5': ('sdk2.family.Path.FIREBASE2', 'particle_family.rs', 'path_effect'),
        '3c9e37': ('sdk2.family.Path.FIREBASE3', 'particle_family.rs', 'path_effect'),
        '999020': ('sdk2.family.Path.FIREBASE4', 'particle_family.rs', 'path_effect'),
        'bec4de': ('sdk2.family.Path.FIREBALL', 'particle_family.rs', 'path_effect'),
        '7c382a': ('sdk2.family.Path.FIREBALL2', 'particle_family.rs', 'path_effect'),
        '567d5c': ('sdk2.family.Path.SLASH', 'particle_family.rs', 'path_effect'),
        '1b3185': ('sdk2.family.Path.VOLT', 'particle_family.rs', 'path_effect'),
        '1f9538': ('sdk2.family.Path.VOLT2', 'particle_family.rs', 'path_effect'),
        '56ed74': ('sdk2.family.Path.VOLT3', 'particle_family.rs', 'path_effect'),
        '14b14f': ('sdk2.family.Path.VOLT4', 'particle_family.rs', 'path_effect'),
        '1551ff': ('sdk2.family.Path.WINDBALL', 'particle_family.rs', 'path_effect'),
        '069aa4': ('sdk2.family.Path.CONCENT', 'particle_family.rs', 'path_effect'),
        '4dd7f1': ('sdk2.family.Path.HOTARU', 'particle_family.rs', 'path_effect'),
        '2c680e': ('sdk2.family.Path.DASH', 'particle_family.rs', 'path_effect'),
        '5e0196': ('sdk2.family.Path.RISING3', 'particle_family.rs', 'path_effect'),
        'c87ddc': ('sdk2.family.Path.RISING4', 'particle_family.rs', 'path_effect'),
        'eb024c': ('sdk2.family.Path.PAPER', 'particle_family.rs', 'path_effect'),
        'b44453': ('sdk2.family.Path.HOTARU_MOVE', 'particle_family.rs', 'path_effect'),
        'a1273e': ('sdk2.family.Path.HOTARU_STAY', 'particle_family.rs', 'path_effect'),
        '8b0d66': ('sdk2.family.Path.SPLASH', 'particle_family.rs', 'path_effect'),
        'e79cf1': ('sdk2.family.Path.RUBBLE', 'particle_family.rs', 'path_effect'),
        '5ba106': ('sdk2.family.Path.TILES', 'particle_family.rs', 'path_effect'),
        'b91ed7': ('sdk2.family.Path.EXPRODE', 'particle_family.rs', 'path_effect'),
        'd05283': ('sdk2.family.Path.STAR', 'particle_family.rs', 'path_effect'),
        'd0d41a': ('sdk2.family.Path.MIRROR', 'particle_family.rs', 'path_effect'),
        '8b8bbe': ('sdk2.family.Path.PRISM_ORBIT', 'particle_family.rs', 'path_effect'),
        '83eda4': ('sdk2.family.Path.PROTECT', 'particle_family.rs', 'path_effect'),
        '33f520': ('sdk2.family.Path.ROUND_Y', 'particle_family.rs', 'path_effect'),
        '8a1583': ('sdk2.family.Path.ROUND_Z', 'particle_family.rs', 'path_effect'),
        '34be97': ('sdk2.family.Path.STAR_LEGACY', 'particle_family.rs', 'path_effect'),
        '37e246': ('sdk2.family.Path.PRISM_PATH', 'particle_family.rs', 'path_effect'),
        '4b7bd7': ('sdk2.family.Path.SPARK', 'particle_family.rs', 'path_effect'),
        'bfe589': ('sdk2.family.Path.SPARK_EX', 'particle_family.rs', 'path_effect'),
        '9b4eec': ('sdk2.family.Path.ERASE', 'particle_family.rs', 'path_effect'),
        '2794bd': ('sdk2.family.Path.ERASE_ATLAS', 'particle_family.rs', 'path_effect'),
        'cf89ee': ('sdk2.family.Path.BASELINE_SPARK', 'particle_family.rs', 'path_effect'),
        '583e86': ('sdk2.family.Path.A_SPARK', 'particle_family.rs', 'path_effect'),
        'dc936c': ('sdk2.family.Path.ARROW', 'particle_family.rs', 'path_effect'),
        '287be0': ('sdk2.family.Path.FALL_SPARK', 'particle_family.rs', 'path_effect'),
        '734ab4': ('sdk2.family.Path.RADIATE', 'particle_family.rs', 'path_effect'),
        'bae261': ('sdk2.family.Path.SEED', 'particle_family.rs', 'path_effect'),
        'daacc2': ('sdk2.family.Path.SEED_ROT', 'particle_family.rs', 'path_effect'),
        'de7f41': ('sdk2.family.Path.PROTECT2', 'particle_family.rs', 'path_effect'),
        '7aff22': ('sdk2.family.Sprite.SpriteProfile::FilmNoise', 'particle_family.rs', 'sprite_effect'),
        '1f6dd7': ('sdk2.family.Sprite.SpriteProfile::Asease', 'particle_family.rs', 'sprite_effect'),
        '0a2e3b': ('sdk2.family.Sprite.SpriteProfile::Common', 'particle_family.rs', 'sprite_effect'),
        '9b1e19': ('sdk2.family.Sprite.SpriteProfile::Dis', 'particle_family.rs', 'sprite_effect'),
        '5c0c3c': ('sdk2.family.Sprite.SpriteProfile::Angry', 'particle_family.rs', 'sprite_effect'),
        '384f72': ('sdk2.family.Sprite.SpriteProfile::Angry', 'particle_family.rs', 'sprite_effect'),
        'fa95fc': ('sdk2.family.Sprite.SpriteProfile::Notice', 'particle_family.rs', 'sprite_effect'),
        '96d268': ('sdk2.family.Sprite.SpriteProfile::Surprise', 'particle_family.rs', 'sprite_effect'),
        'd23634': ('sdk2.family.Sprite.SpriteProfile::Kyupin', 'particle_family.rs', 'sprite_effect'),
        'd369d7': ('sdk2.family.Sprite.SpriteProfile::Silent', 'particle_family.rs', 'sprite_effect'),
        '447d65': ('sdk2.family.Sprite.SpriteProfile::Down', 'particle_family.rs', 'sprite_effect'),
        'de2345': ('sdk2.family.Sprite.SpriteProfile::Exclamation', 'particle_family.rs', 'sprite_effect'),
        'edc067': ('sdk2.family.Sprite.SpriteProfile::FlowerBurst', 'particle_family.rs', 'sprite_effect'),
        '1bab19': ('sdk2.family.Sprite.SpriteProfile::Ring', 'particle_family.rs', 'sprite_effect'),
        'e2a604': ('sdk2.family.Sprite.SpriteProfile::Digital', 'particle_family.rs', 'sprite_effect'),
        '418b15': ('sdk2.family.Sprite.SpriteProfile::Digital', 'particle_family.rs', 'sprite_effect'),
        '273955': ('sdk2.family.Sprite.SpriteProfile::Double', 'particle_family.rs', 'sprite_effect'),
        '5d207c': ('sdk2.family.Sprite.SpriteProfile::Double', 'particle_family.rs', 'sprite_effect'),
    }
    layout_source = (SOURCE/'particle_plugin_layout.rs').read_text(encoding='utf8')
    for prefix, (model, name, detector) in layouts.items():
        matches = [sha for sha in inventory['groups'] if sha.startswith(prefix)]
        if len(matches) != 1 or not re.search(rf'\bfn {detector}\(', layout_source):
            raise ValueError(f'PPI report evidence unresolved: {prefix} / {detector}')
        models[matches[0]] = (model, name)
    described = (SOURCE/'particle_family_profiles.rs').read_text(encoding='utf8')
    for sha,kind,profile in re.findall(r'Profile\s*\{\s*sha256:\s*"([a-f0-9]{64})",\s*model:\s*Model::(\w+)(?:\(([\w:]+))?',described):
        models[sha] = (f'sdk2.family.{kind}'+(f'.{profile}' if profile else ''),'particle_family.rs')
    rows = []
    for sha,item in inventory['groups'].items():
        identity = classification[sha]
        active = sha in models
        structural = sha[:6] in layouts
        sdk1 = identity['version'] == 0x10000
        model,consumer = models.get(sha,(None,None))
        evidence=['archive/2026-11/20261117_D_ppi_cfg_clusters.json']
        if structural and model in {
            'sdk2.family.Path.' + name for name in
            ('NOTE', 'OKO', 'SWEAT', 'SIGH', 'ENERGY', 'KIRAKIRA', 'WARUKIRA', 'SHY', 'STEAM', 'WC_STEAM')
        }:
            evidence.append('archive/2026-11/20261125_ppi_path_factory_parameters.txt')
        if structural and model in {
            'sdk2.family.Path.' + name for name in
            ('FIREBALL', 'FIREBALL2', 'SLASH', 'VOLT', 'VOLT2', 'VOLT3', 'VOLT4', 'WINDBALL')
        }:
            evidence.append('archive/2026-11/20261125_ppi_fire_factory_callbacks.txt')
        if structural and model in {
            'sdk2.family.Path.' + name for name in
            ('CONCENT', 'HOTARU', 'DASH', 'RISING3', 'RISING4', 'PAPER', 'HOTARU_MOVE', 'HOTARU_STAY')
        }:
            evidence.append('archive/2026-11/20261125_ppi_priority_path_callbacks.txt')
        if structural and model in {
            'sdk2.family.Path.' + name for name in
            ('SPLASH', 'RUBBLE', 'TILES', 'EXPRODE', 'STAR', 'MIRROR', 'PRISM_ORBIT', 'PROTECT', 'ROUND_Y', 'ROUND_Z')
        }:
            evidence.append('archive/2026-11/20261125_ppi_priority_pool_callbacks.txt')
            if model.endswith(('.MIRROR', '.PRISM_ORBIT', '.ROUND_Y', '.ROUND_Z')):
                evidence.append('archive/2026-11/20261125_ppi_priority_pool_spawn.txt')
        if structural and model in {
            'sdk2.family.Path.' + name for name in
            ('STAR_LEGACY', 'PRISM_PATH', 'SPARK', 'SPARK_EX', 'ERASE', 'ERASE_ATLAS', 'BASELINE_SPARK',
             'A_SPARK', 'ARROW', 'FALL_SPARK', 'RADIATE', 'SEED', 'SEED_ROT', 'PROTECT2')
        }:
            evidence.append('archive/2026-11/20261125_ppi_remaining_callbacks.txt')
            if model.endswith(('.SPARK', '.SPARK_EX')):
                evidence.append('archive/2026-11/20261125_ppi_spark_tangent.txt')
            if model.endswith(('.ERASE', '.ERASE_ATLAS')):
                evidence.append('archive/2026-11/20261125_ppi_spark_erase_generators.txt')
        if sdk1:
            evidence.append('archive/2026-11/20261117_D_ptclpi_host_loader.txt:14e7a6..14e7c9 (type1; versions20000/20001/20002)')
        elif active and model=='sdk2.common_plist':
            evidence.extend(['archive/2026-11/20261117_D_ppi_config_'+sha[:10]+'.txt',
                'archive/2026-11/20261117_D_ppi_common_compare.txt'])
        elif active and model.endswith(('.MESSAGE','.MESSAGE_RANDOM')):
            evidence.extend(['archive/2026-11/20261117_D_ppi_remaining_sprite_'+sha[:10]+'.txt',
                'archive/2026-11/20261117_D_ppi_message_tables.json'])
        elif active and model.endswith(('.TEXT_WORDS','.STREAM_ARROW','.CUBE','.LIGHT','.RIBBON','.WIND_TRAIL','.RADIAL_FLARE','.DAMAGE','.EARLY_SQUARE','.CIRCLE_LIGHT','.CIRCLE_LIGHT_TUNED','.HAPPY','.PHOTON','.DARK','.WC_FALL','.HEART','.INVISI','.BUBBLE','.NUMBER','.ROTATING_BACKGROUND','.BLUR_STRIPS')):
            evidence.append('archive/2026-11/20261117_D_ppi_remaining_sprite_'+sha[:10]+'.txt')
            if model.endswith('.TEXT_WORDS'):
                evidence.append('archive/2026-11/20261117_D_ppi_text_config.json')
            if model.endswith('.CUBE'):
                evidence.extend(['archive/2026-11/20261117_D_ppi_cube_data.json','archive/2026-11/20261117_D_ppi_priority2_callbacks.txt'])
            if model.endswith('.LIGHT'):
                evidence.append('archive/2026-11/20261117_D_ppi_light_atlas.json')
            if model.endswith('.RADIAL_FLARE'):
                evidence.append('archive/2026-11/20261117_D_ppi_radial_atlas.json')
            if model.endswith('.HEART'):
                evidence.append('archive/2026-11/20261117_D_ppi_heart_tables.json')
            if model.endswith(('.BUBBLE','.NUMBER')):
                evidence.append('archive/2026-11/20261117_D_ppi_atlas_emitter_tables.json')
        elif active and model.endswith(('.THUNDER','.FALL_SPARK')):
            evidence.extend(['archive/2026-11/20261117_D_ppi_priority2_'+sha[:10]+'.txt',
                'archive/2026-11/20261117_D_ppi_thunder_data.txt'])
        elif active and model.endswith(('.DUST','.TWINKLE','.FLOATING_SPARK','.A_SPARK')):
            evidence.extend(['archive/2026-11/20261117_D_ppi_priority2_'+sha[:10]+'.txt',
                'archive/2026-11/20261117_D_ppi_flicker_callback.txt'])
        elif active and model.endswith(('.FIRE_PATTERN','.POND','.VORTEX','.BASELINE_SPARK','.TORCH','.SMOKE')):
            evidence.append('archive/2026-11/20261117_D_ppi_geometry_'+sha[:10]+'.txt')
            if model.endswith('.SMOKE'):
                evidence.append('archive/2026-11/20261117_D_ppi_smoke_data.txt')
        elif active and model.endswith(('.SWORD_SPARK','.CREATE_SQUARE','.CONCENT_O','.ROLLING','.ICE','.ICE2')):
            evidence.append('archive/2026-11/20261117_D_ppi_matrix_'+sha[:10]+'.txt')
            if model.endswith(('.ICE','.ICE2')):
                evidence.append('archive/2026-11/20261117_D_ppi_ice_data.txt')
        elif active and model.endswith(('.PROTECT','.PROTECT2')):
            evidence.extend(['archive/2026-11/20261117_D_ppi_geometry_'+sha[:10]+'.txt',
                'archive/2026-11/20261117_D_ppi_protect_tables.json',
                'archive/2026-11/20261117_D_ppi_thunder_data.txt'])
        elif active and model.endswith(('.RADIATE','.ARROW')):
            evidence.append('archive/2026-11/20261117_D_ppi_priority2_'+sha[:10]+'.txt')
        elif active and model.endswith(('.SEED','.SEED_ROT')):
            evidence.extend(['archive/2026-11/20261117_D_ppi_priority2_'+sha[:10]+'.txt',
                'archive/2026-11/20261117_D_ppi_seed_tables.json'])
        elif active and '.Path.' in model:
            if model.endswith(('.TILES','.MIRROR','.EXPRODE','.FIREBASE','.FIREBASE2','.FIREBASE3','.FIREBASE4','.SPARK','.SPARK_EX','.RUBBLE','.ROUND_Y','.ROUND_Z','.ERASE','.ERASE_ATLAS','.STAR','.STAR_LEGACY','.PRISM_PATH','.PRISM_ORBIT','.PRISM_TRAIL')):
                evidence.extend(['archive/2026-11/20261117_D_ppi_priority2_'+sha[:10]+'.txt',
                    'archive/2026-11/20261117_D_ppi_priority2_callbacks.txt',
                    'archive/2026-11/20261116_D_particle669_object_disassembly.txt:14e30f/14e58a'])
            elif model.endswith(('.FIREBALL','.FIREBALL2','.SLASH','.HADOU','.HOTARU_MOVE','.HOTARU_STAY','.WINDBALL','.SPLASH')):
                evidence.append('archive/2026-11/20261117_D_ppi_fire_'+sha[:10]+'.txt')
            elif model.endswith(('.VOLT','.VOLT2','.VOLT3','.VOLT4')):
                evidence.append('archive/2026-11/20261117_D_ppi_volt_'+sha[:10]+'.txt')
            elif model.endswith(('.SIGH','.SHY','.KIRAKIRA','.OKO','.WARUKIRA','.CONCENT','.HOTARU','.ENERGY','.STEAM','.WC_STEAM','.DASH','.RISING3','.RISING4','.SAKURA','.PAPER')):
                evidence.append('archive/2026-11/20261117_D_ppi_followup_'+sha[:10]+'.txt')
            else:
                evidence.append('archive/2026-11/20261117_D_ppi_path_evidence.json')
        elif active and '.Sprite.' in model:
            if model.endswith('::Angry'):
                evidence.extend(['archive/2026-11/20261117_D_ppi_angry_evidence.json','archive/2026-11/20261117_D_ppi_math_callbacks.txt'])
            else:
                evidence.append('archive/2026-11/20261117_D_ppi_'+('priority_' if sha.startswith(('fa95fc','1f6dd7','0a2e3b')) else 'followup_')+sha[:10]+'.txt')
        routes = None
        if active:
            routes = {
                'identity_gate':(
                    'simulator/crates/desktop/src/particle_plugin_layout.rs::'+layouts[sha[:6]][2]
                    if structural else 'simulator/crates/desktop/src/particle_plugin.rs::ParticlePlugin::load'
                ),
                'model_source':'simulator/crates/desktop/src/'+consumer,
                'initialize':'ParticlePlugin::start'+(' -> Family::start' if consumer=='particle_family.rs' else ''),
                'update':'LoggingHost::advance_particle_task -> ParticlePlugin::update'+(' -> Family::update' if consumer=='particle_family.rs' else ''),
                'snapshot':'ParticlePlugin::snapshot'+(' -> Family::snapshot' if consumer=='particle_family.rs' else ''),
                'render':'HeadlessPlayer::render_scene_nodes_range -> particle_render::draw_range_with_projection / particle_external::submit',
                'stop':'ParticlePlugin::stop removes active snapshot drawing',
                'release':'LoggingHost::release_particle_program drops plugin, texture and snapshot; reload replaces the owned instance',
            }
            if '.Path.' in model:
                routes['model_source']='simulator/crates/desktop/src/particle_family_path.rs::PathFamily'
                routes['initialize']+=' -> PathFamily::start'
                routes['update']+=' -> PathFamily::update'
                routes['snapshot']+=' -> PathFamily::snapshot'
            if model.endswith('.TEXT_WORDS'):
                routes['initialize']+=' -> PathFamily::load_text_config -> ResourceIndex::read_mode -> real plist word configuration'
            if '.Sprite.' in model:
                routes['model_source']='simulator/crates/desktop/src/particle_family_sprite.rs::SpriteFamily'
                routes['initialize']+=' -> SpriteFamily::start'
                routes['update']+=' -> SpriteFamily::update'
                routes['snapshot']+=' -> SpriteFamily::snapshot'
            if model=='sdk2.common_plist':
                routes['initialize']+=' -> CommonParticles::start -> ResourceIndex::read_mode -> plist scalar configuration'
                routes['update']+=' -> CommonParticles::update'
                routes['snapshot']+=' -> CommonParticles::snapshot'
        u = usage.get(sha,{})
        rows.append({
            'sha256':sha,'model':'sdk2.host-reject.sdk1' if sdk1 else model,'active_source_consumer':{'reject':'ParticlePlugin::load -> registered SDK type and version'} if sdk1 else routes,
            'equivalence_status':('source model registered by SDK callback structure' if structural else 'source model registered by exact SHA')+'; full behavioral/pixel equivalence has not been established' if active else 'not established',
            'model_boundaries':(['wc_torch2720 reads object64/68 angular fields not initialized by public1f60/2050; independent model initializes unspecified storage to zero, consumes the real RNG call, and does not claim allocator-dependent equivalence.'] if model and model.endswith('.TORCH') else
                ['Real ptcl%d%03d.plist resource is required at start; missing/archive-unresolved configuration fails explicitly. Consumed numeric fields must be present; uninitialized original allocator fields are not synthesized. Flat XML plist scalar records are supported; binary/nested plist is diagnosed. Scene acceptance has not been verified.'] if model=='sdk2.common_plist' else ['wc_number start/update do not consume configured size or integrate sampled private velocity. Uninitialized object80/48/4c arithmetic does not affect its reachable glyph/phase consumer; allocator-dependent arithmetic equivalence is not claimed.'] if model and model.endswith('.NUMBER') else []),
            'remaining':not active and not sdk1,
            'algorithm_implemented':active,
            'verified_rejection':{'source':'ParticlePlugin::load -> registered SDK type and version','type':1,'version':65536,'reason':'Fixed SDK2 host branches around MakeInstance and destroys the unloaded wrapper for SDK1.'} if sdk1 else None,
            'remaining_reason':None if active or sdk1 else (
                'SDK1 algorithm not implemented; fixed SDK2 host rejects version0x10000 before MakeInstance. Rejection is not algorithm coverage.' if sdk1 else
                'No independent initialized/updated/rendered effect model for this exact identity; explicit unsupported diagnostic remains.'),
            'status':'implemented-source-route' if active else ('fixed-sdk2-loader-reject' if sdk1 else 'unimplemented-sdk2'),
            'type':identity['type'],'version':identity['version'],
            'paths':item['paths'],'file_copies':len(item['paths']),
            'method_rvas':[m['entry'] for m in cfg[sha]['methods']],
            'evidence':evidence,
            'priority_evidence':{
                'lexical_kcs_initializer_matches':u.get('initializer_resource_count',0),
                'same_basename_kcs_assets':u.get('named_kcs_resource_count',0),
                'explicit_ppi_string_occurrences':sum(r['explicit_ppi_occurrences'] for r in u.get('initializer_references',[])),
                'static_effect_command_records':u.get('effect_command_count',0),
                'example_effect_commands':u.get('effect_command_references',[])[:2],
                'scope':'Static scene/FES effect-name records and lexical KCS/resource-name correlation, not execution frequency or proof of startup reachability; generic names such as blur/steam can collide.',
            },
        })
    result = {
        'schema':'cs2-particle-ppi-source-manifest/v1',
        'verification_scope':'Static inventory, PE data and source routing only. No compile/type-check/test/gameplay/VM/plugin/PE execution. Implemented source route is not game acceptance.',
        'refresh':'python scripts/analysis/particle_ppi_manifest.py',
        'summary':{
            'identities':len(rows),'files':sum(r['file_copies'] for r in rows),
            'duplicate_file_copies':sum(r['file_copies'] for r in rows)-len(rows),
            'implemented_source_routes':sum(r['status']=='implemented-source-route' for r in rows),
            'accounted_identities':sum(not r['remaining'] for r in rows),
            'remaining':sum(r['remaining'] for r in rows),
            'remaining_sdk2':sum(r['status']=='unimplemented-sdk2' for r in rows),
            'fixed_sdk2_rejected_sdk1':sum(r['status']=='fixed-sdk2-loader-reject' for r in rows),
        },
        'shared_boundaries':[
            f'{len(layouts)} identities use SDK callback structure. The remaining live model dispatches retain exact resource SHA. Checkpoint resource identity is checked separately.',
            'Existing Canvas/external renderer, 3D source lookup, texture routing and geometry support keep their own unresolved boundaries.',
            'Aliases produced dynamically by scripts require separate call-argument/dataflow evidence; absence of a lexical match does not make an effect unused.',
        ],
        'rows':sorted(rows,key=lambda r:r['sha256']),
    }
    target=ROOT/'reports/particle_ppi_manifest_20261117.json'
    target.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf8')
    print(json.dumps(result['summary'],indent=2))


if __name__=='__main__':
    main()
