package com.example.chunkcourier;

import net.minecraftforge.event.RegisterCommandsEvent;
import net.minecraftforge.event.TickEvent;
import net.minecraftforge.event.entity.player.PlayerInteractEvent;
import net.minecraftforge.event.level.BlockEvent;
import net.neoforged.bus.api.SubscribeEvent;
import net.neoforged.fml.common.Mod;

@Mod("chunkcourier")
public class ChunkCourier {
    private int ticks;

    @SubscribeEvent
    public void onBreak(BlockEvent.BreakEvent event) {
        event.setCanceled(false);
    }

    @SubscribeEvent
    public void onEntityInteract(PlayerInteractEvent.EntityInteract event) {
        event.setCanceled(false);
    }

    @SubscribeEvent
    public void onPlayerTick(TickEvent.PlayerTickEvent event) {
        if (event.player == null) {
            return;
        }
        ticks++;
        if (ticks % 20 == 0) {
            event.player.sendSystemMessage("chunkcourier: parcels ticked " + ticks);
        }
    }

    @SubscribeEvent
    public void onRegisterCommands(RegisterCommandsEvent event) {
        event.getDispatcher().register(new CourierCommand());
    }

    public static class CourierCommand {
    }
}
